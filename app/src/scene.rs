//! Software 3D preview: flat-shaded faces rasterized on the CPU into a depth
//! buffer, so the nearest surface wins at every pixel from any angle. Holes
//! (windows, doors) are separate loops of a face and stay open through
//! even-odd filling. The picture is painted as one image.

use aufmass::model::{Building, Kind};
use gpui::{px, rgb, size, Bounds, Corners, Pixels, RenderImage, Rgba, Window};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub const SLAB_COLOR: u32 = 0x9fb4c7;
pub const ROOF_COLOR: u32 = 0xb5654f;

/// Marks an empty pixel in the face and edge buffers.
const NONE: u32 = u32::MAX;

struct Face {
    loops: Vec<Vec<[f32; 3]>>,
    normal: [f32; 3],
    color: Rgba,
    storey: usize,
    kind: Kind,
}

pub struct Scene {
    faces: Vec<Face>,
    center: [f32; 3],
    radius: f32,
    cache: Mutex<Cache>,
}

/// The last picture and the buffers it was drawn in, kept between frames.
#[derive(Default)]
struct Cache {
    view: Option<View>,
    image: Option<Arc<RenderImage>>,
    raster: Raster,
}

/// Everything a picture depends on; the same view reuses the last one.
#[derive(Clone, PartialEq)]
struct View {
    camera: Camera,
    visibility: Visibility,
    /// Size of the image in device pixels.
    width: u32,
    height: u32,
    /// Drawn at this multiple of the size and averaged down, for smooth edges.
    supersample: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { yaw: -0.65, pitch: 0.55, zoom: 1.3 }
    }
}

/// What the 3D view shows.
#[derive(Clone, PartialEq)]
pub struct Visibility {
    pub storeys: Vec<bool>,
    pub roof: bool,
}

impl Scene {
    pub fn new(b: &Building, wall_colors: &BTreeMap<String, &'static str>) -> Scene {
        let mut faces = Vec::new();
        for (si, s) in b.storeys.iter().enumerate() {
            for e in &s.elements {
                let color = match e.kind {
                    Kind::Wall => wall_colors.get(&e.name).map_or(0x888888, |c| hex(c)),
                    Kind::Slab => SLAB_COLOR,
                    Kind::Roof => ROOF_COLOR,
                };
                for f in &e.faces {
                    let loops: Vec<Vec<[f32; 3]>> =
                        f.iter().map(|l| l.iter().map(|p| [p.x as f32, p.y as f32, p.z as f32]).collect()).collect();
                    // ACIS orders the outer loop counter-clockwise seen from outside,
                    // so its normal points out of the solid.
                    let Some(outer) = loops.iter().max_by(|a, b| newell(a).1.total_cmp(&newell(b).1)) else { continue };
                    let normal = newell(outer).0;
                    faces.push(Face { normal, loops, color: rgb(color), storey: si, kind: e.kind });
                }
            }
        }
        Scene::with_faces(faces)
    }

    fn with_faces(faces: Vec<Face>) -> Scene {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in faces.iter().flat_map(|f| f.loops.iter().flatten()) {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let radius = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() / 2.0;
        Scene { faces, center, radius: radius.max(1.0), cache: Mutex::default() }
    }

    pub fn paint(&self, cam: Camera, vis: &Visibility, bounds: Bounds<Pixels>, window: &mut Window) {
        let scale_factor = window.scale_factor();
        let width = (f32::from(bounds.size.width) * scale_factor).ceil() as u32;
        let height = (f32::from(bounds.size.height) * scale_factor).ceil() as u32;
        if width == 0 || height == 0 {
            return;
        }
        // Retina screens are fine as they are; standard ones get 2×2 samples per pixel.
        let supersample = if scale_factor < 1.5 { 2 } else { 1 };
        let view = View { camera: cam, visibility: vis.clone(), width, height, supersample };
        let mut cache = self.cache.lock().unwrap();
        let cache = &mut *cache;
        if cache.view.as_ref() != Some(&view) {
            let image = Arc::new(self.render(&view, &mut cache.raster));
            // A new image per camera move; the old one would otherwise stay in GPUI's atlas.
            if let Some(old) = cache.image.replace(image) {
                window.drop_image(old).ok();
            }
            cache.view = Some(view);
        }
        let Some(image) = cache.image.clone() else { return };
        let size = size(px(width as f32 / scale_factor), px(height as f32 / scale_factor));
        window.paint_image(Bounds::new(bounds.origin, size), Corners::default(), image, 0, false).ok();
    }

    fn render(&self, view: &View, raster: &mut Raster) -> RenderImage {
        self.rasterize(view, raster);
        let data = raster.resolve(&self.faces, view);
        let buffer = image::RgbaImage::from_raw(view.width, view.height, data).expect("image buffer size");
        RenderImage::new(vec![image::Frame::new(buffer)])
    }

    /// Fills the depth, face and edge buffers for `view`.
    fn rasterize(&self, view: &View, raster: &mut Raster) {
        let cam = view.camera;
        let ss = view.supersample as usize;
        let (w, h) = (view.width as usize * ss, view.height as usize * ss);
        raster.clear(w, h);
        let scale = w.min(h) as f32 / (2.1 * self.radius) * cam.zoom;
        let (sin_yaw, cos_yaw) = cam.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = cam.pitch.sin_cos();
        let c = self.center;
        // Rotate around z by yaw, then tilt by pitch; returns pixel x, y and depth
        // (larger = farther), all in pixels of the raster.
        let project = |p: &[f32; 3]| {
            let (x, y, z) = (p[0] - c[0], p[1] - c[1], p[2] - c[2]);
            let x1 = x * cos_yaw - y * sin_yaw;
            let y1 = x * sin_yaw + y * cos_yaw;
            let up = y1 * sin_pitch + z * cos_pitch;
            let depth = y1 * cos_pitch - z * sin_pitch;
            [w as f32 / 2.0 + x1 * scale, h as f32 / 2.0 - up * scale, depth * scale]
        };
        // Direction from the scene toward the camera; the opposite of the depth gradient.
        let to_camera = [-sin_yaw * cos_pitch, -cos_yaw * cos_pitch, sin_pitch];

        // Back faces of the closed solids are always hidden; skipping them halves the work.
        let visible: Vec<(u32, Vec<Vec<[f32; 3]>>)> = self
            .faces
            .iter()
            .enumerate()
            .filter(|(_, f)| view.visibility.storeys.get(f.storey).copied().unwrap_or(true))
            .filter(|(_, f)| view.visibility.roof || f.kind != Kind::Roof)
            .filter(|(_, f)| dot(f.normal, to_camera) > -0.02)
            .map(|(i, f)| (i as u32, f.loops.iter().map(|l| l.iter().map(project).collect()).collect()))
            .collect();
        // Edges need the finished depth buffer to know which of them are hidden.
        for (id, loops) in &visible {
            raster.fill(loops, *id);
        }
        for (id, loops) in &visible {
            for l in loops {
                for (k, a) in l.iter().enumerate() {
                    raster.edge(*a, l[(k + 1) % l.len()], *id);
                }
            }
        }
    }
}

/// Per-pixel buffers at the supersampled size.
#[derive(Default)]
struct Raster {
    width: usize,
    height: usize,
    depth: Vec<f32>,
    /// Index of the nearest face, or `NONE`.
    face: Vec<u32>,
    /// Face whose outline crosses the pixel, or `NONE`.
    edge: Vec<u32>,
    /// Scratch for the span ends of one row.
    xs: Vec<f32>,
}

impl Raster {
    fn clear(&mut self, width: usize, height: usize) {
        let n = width * height;
        (self.width, self.height) = (width, height);
        self.depth.clear();
        self.depth.resize(n, f32::INFINITY);
        self.face.clear();
        self.face.resize(n, NONE);
        self.edge.clear();
        self.edge.resize(n, NONE);
    }

    /// Fills a planar face given in raster coordinates, even-odd over all its
    /// loops, keeping the nearest face per pixel.
    fn fill(&mut self, loops: &[Vec<[f32; 3]>], id: u32) {
        let Some(&origin) = loops.first().and_then(|l| l.first()) else { return };
        // The face is planar, so its depth is a linear function of x and y.
        // Holes run the other way round, which only shrinks the summed normal.
        let mut n = [0.0f32; 3];
        for l in loops {
            let (m, _) = newell_raw(l);
            (0..3).for_each(|k| n[k] += m[k]);
        }
        if n[2].abs() <= 1e-4 * dot(n, n).sqrt() {
            return; // Seen edge-on.
        }
        let (dzdx, dzdy) = (-n[0] / n[2], -n[1] / n[2]);
        let (lo, hi) = loops.iter().flatten().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
        let rows = (lo - 0.5).ceil().clamp(0.0, self.height as f32) as usize..(hi - 0.5).ceil().clamp(0.0, self.height as f32) as usize;
        for row in rows {
            // Sample at pixel centers.
            let y = row as f32 + 0.5;
            self.xs.clear();
            for l in loops {
                for (k, a) in l.iter().enumerate() {
                    let b = l[(k + 1) % l.len()];
                    if (a[1] <= y) != (b[1] <= y) {
                        self.xs.push(a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                    }
                }
            }
            self.xs.sort_by(f32::total_cmp);
            for span in self.xs.chunks_exact(2) {
                let start = (span[0] - 0.5).ceil().clamp(0.0, self.width as f32) as usize;
                let end = (span[1] - 0.5).ceil().clamp(0.0, self.width as f32) as usize;
                let mut z = origin[2] + dzdx * (start as f32 + 0.5 - origin[0]) + dzdy * (y - origin[1]);
                let line = row * self.width;
                for i in line + start..line + end {
                    if z < self.depth[i] {
                        self.depth[i] = z;
                        self.face[i] = id;
                    }
                    z += dzdx;
                }
            }
        }
    }

    /// Marks the pixels of one outline segment of face `id` that are not
    /// hidden behind another face.
    fn edge(&mut self, a: [f32; 3], b: [f32; 3], id: u32) {
        let (w, h) = (self.width as f32, self.height as f32);
        let Some((t0, t1)) = clip(a, b, w, h) else { return };
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let steps = ((d[0].abs().max(d[1].abs()) * (t1 - t0)).ceil() as usize).max(1);
        for s in 0..=steps {
            let t = t0 + (t1 - t0) * s as f32 / steps as f32;
            let (x, y) = ((a[0] + d[0] * t).floor(), (a[1] + d[1] * t).floor());
            if x < 0.0 || y < 0.0 || x >= w || y >= h {
                continue;
            }
            let i = y as usize * self.width + x as usize;
            // On its own face, or in front of whatever covers the pixel. The
            // small tolerance keeps edges shared with a neighbouring face.
            if self.face[i] == id || a[2] + d[2] * t <= self.depth[i] + 0.5 {
                self.edge[i] = id;
            }
        }
    }

    /// The finished picture: shaded faces with darker outlines, averaged down
    /// to the view's size, as GPUI's BGRA with straight alpha. Empty pixels
    /// stay transparent.
    fn resolve(&self, faces: &[Face], view: &View) -> Vec<u8> {
        let light = normalize([0.35, -0.55, 0.75]);
        let fill: Vec<[f32; 3]> = faces
            .iter()
            .map(|f| {
                let shade = 0.6 + 0.4 * dot(f.normal, light).max(0.0);
                [f.color.r * shade, f.color.g * shade, f.color.b * shade]
            })
            .collect();
        const EDGE_ALPHA: f32 = 0.55;
        let ss = view.supersample as usize;
        let (w, h) = (view.width as usize, view.height as usize);
        let samples = (ss * ss) as f32;
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                // Premultiplied sums over the samples of this pixel.
                let mut sum = [0.0f32; 4];
                for sy in 0..ss {
                    for sx in 0..ss {
                        let i = (y * ss + sy) * self.width + x * ss + sx;
                        let mut c = match self.face[i] {
                            NONE => [0.0; 4],
                            f => {
                                let [r, g, b] = fill[f as usize];
                                [r, g, b, 1.0]
                            }
                        };
                        if self.edge[i] != NONE {
                            let line = faces[self.edge[i] as usize].color;
                            let line = [line.r * 0.35, line.g * 0.35, line.b * 0.35, 1.0];
                            c = [0, 1, 2, 3].map(|k| line[k] * EDGE_ALPHA + c[k] * (1.0 - EDGE_ALPHA));
                        }
                        (0..4).for_each(|k| sum[k] += c[k]);
                    }
                }
                if sum[3] > 0.0 {
                    let to_byte = |v: f32| (v / sum[3] * 255.0).round().clamp(0.0, 255.0) as u8;
                    let o = (y * w + x) * 4;
                    out[o..o + 4].copy_from_slice(&[to_byte(sum[2]), to_byte(sum[1]), to_byte(sum[0]), (sum[3] / samples * 255.0).round() as u8]);
                }
            }
        }
        out
    }
}

/// The part of segment `a`–`b` inside a `w`×`h` raster as a parameter range,
/// so far off-screen edges of a zoomed-in model cost nothing.
fn clip(a: [f32; 3], b: [f32; 3], w: f32, h: f32) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-(b[0] - a[0]), a[0]), (b[0] - a[0], w - a[0]), (-(b[1] - a[1]), a[1]), (b[1] - a[1], h - a[1])] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

/// "#c0601e" -> 0xc0601e
pub fn hex(color: &str) -> u32 {
    u32::from_str_radix(color.trim_start_matches('#'), 16).unwrap_or(0x888888)
}

/// Unit normal and twice the area of a planar loop.
fn newell(pts: &[[f32; 3]]) -> ([f32; 3], f32) {
    let (n, area) = newell_raw(pts);
    (normalize(n), area)
}

/// Normal of a planar loop with twice its area as length, and that length.
fn newell_raw(pts: &[[f32; 3]]) -> ([f32; 3], f32) {
    let mut n = [0.0f32; 3];
    for (i, a) in pts.iter().enumerate() {
        let b = pts[(i + 1) % pts.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    (n, dot(n, n).sqrt())
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt();
    if l > 0.0 { [v[0] / l, v[1] / l, v[2] / l] } else { v }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square facing -y (toward a camera with yaw and pitch 0) at depth `y`,
    /// with a square hole in the middle when `hole` is set.
    fn square(y: f32, half: f32, hole: bool, color: u32) -> Face {
        let outer = vec![[-half, y, -half], [half, y, -half], [half, y, half], [-half, y, half]];
        let mut loops = vec![outer];
        if hole {
            let q = half / 3.0;
            loops.push(vec![[-q, y, -q], [-q, y, q], [q, y, q], [q, y, -q]]);
        }
        let normal = newell(&loops[0]).0;
        Face { loops, normal, color: rgb(color), storey: 0, kind: Kind::Wall }
    }

    fn view() -> View {
        let camera = Camera { yaw: 0.0, pitch: 0.0, zoom: 1.0 };
        View { camera, visibility: Visibility { storeys: vec![true], roof: true }, width: 100, height: 100, supersample: 1 }
    }

    fn center_face(faces: Vec<Face>) -> u32 {
        let scene = Scene::with_faces(faces);
        let mut raster = Raster::default();
        scene.rasterize(&view(), &mut raster);
        raster.face[50 * raster.width + 50]
    }

    #[test]
    fn nearer_face_wins_whatever_the_order() {
        // Squares facing the camera, the large one behind the small one.
        assert_eq!(center_face(vec![square(0.0, 1.0, false, 0x0000ff), square(-1.0, 0.5, false, 0xff0000)]), 1);
        assert_eq!(center_face(vec![square(-1.0, 0.5, false, 0xff0000), square(0.0, 1.0, false, 0x0000ff)]), 0);
    }

    #[test]
    fn holes_show_what_is_behind() {
        assert_eq!(center_face(vec![square(-1.0, 1.0, true, 0xff0000), square(0.0, 1.0, false, 0x0000ff)]), 1);
    }

    #[test]
    fn empty_pixels_are_transparent() {
        let scene = Scene::with_faces(vec![square(0.0, 1.0, false, 0xff0000)]);
        let view = view();
        let data = scene.render(&view, &mut Raster::default()).as_bytes(0).unwrap().to_vec();
        let pixel = |x: usize, y: usize| &data[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4];
        assert_eq!(pixel(0, 0)[3], 0);
        // BGRA: the red square has its red in the third byte.
        assert_eq!(pixel(50, 50)[3], 255);
        assert!(pixel(50, 50)[2] > 200 && pixel(50, 50)[0] == 0);
    }
}
