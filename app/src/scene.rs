//! Software 3D preview: flat-shaded faces rasterized on the CPU into a depth
//! buffer, so the nearest surface wins at every pixel from any angle. Holes
//! (windows, doors) are separate loops of a face and stay open through
//! even-odd filling. The picture is painted as one image, the dimensions on
//! top of it (`dimension.rs`).

use crate::dimension::{self, Frame, Style};
use aufmass::geom::V3;
use aufmass::model::{Building, Element, Kind};
use gpui::{px, rgb, size, App, Bounds, Corners, Pixels, Point, RenderImage, Rgba, Size, Window};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub const SLAB_COLOR: u32 = 0x9fb4c7;
pub const ROOF_COLOR: u32 = 0xb5654f;
/// Behind the model, and behind the dimension values so no line runs through them.
pub const BACKGROUND: u32 = 0xfafbfc;
/// The selected element and its dimensions: the app's accent.
pub const SELECTED: u32 = 0x0000bd;
/// The dimensions of the whole model.
pub const DIMENSION: u32 = 0x2a2f3a;

/// Room kept around the model for its dimensions, in pixels, so they stay in
/// view in a small window too.
const ROOM: f32 = 100.0;

/// Marks an empty pixel in the face and edge buffers.
const NONE: u32 = u32::MAX;

struct Face {
    loops: Vec<Vec<[f32; 3]>>,
    normal: [f32; 3],
    color: Rgba,
    storey: usize,
    kind: Kind,
    /// Index into `Scene::items`.
    item: u32,
}

/// A building element, for picking and dimensioning.
struct Item {
    /// Storey and position in it, as in the building.
    storey: usize,
    index: usize,
    kind: Kind,
    frame: Option<Frame>,
}

pub struct Scene {
    faces: Vec<Face>,
    items: Vec<Item>,
    /// The direction most elements run along in plan, for the overall dimensions.
    plan_axis: [f32; 2],
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
    /// Where the picture was painted, and raster pixels per logical pixel, for picking.
    placement: Option<(Point<Pixels>, f32)>,
}

/// Everything a picture depends on; the same view reuses the last one.
#[derive(Clone, PartialEq)]
struct View {
    camera: Camera,
    visibility: Visibility,
    selected: Option<usize>,
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
    /// Moves the picture by this many pixels.
    pub pan: [f32; 2],
}

impl Default for Camera {
    fn default() -> Self {
        Self { yaw: -0.65, pitch: 0.55, zoom: 1.5, pan: [0.0, 0.0] }
    }
}

/// How a flat view, a floor plan or an elevation, is shown: moved by `pan`
/// pixels and enlarged by `zoom`.
#[derive(Clone, Copy, PartialEq)]
pub struct PanZoom {
    pub pan: [f32; 2],
    pub zoom: f32,
}

impl Default for PanZoom {
    fn default() -> Self {
        Self { pan: [0.0, 0.0], zoom: 1.0 }
    }
}

impl PanZoom {
    /// Zooms by `factor` around the middle of the view.
    pub fn zoom_by(&mut self, factor: f32) {
        let zoom = (self.zoom * factor).clamp(0.3, 20.0);
        let k = zoom / self.zoom;
        self.pan = [self.pan[0] * k, self.pan[1] * k];
        self.zoom = zoom;
    }
}

/// What the model view shows, in 3D or as a floor plan.
#[derive(Clone, PartialEq)]
pub struct Visibility {
    pub storeys: Vec<bool>,
    pub roof: bool,
    pub dimensions: bool,
}

impl Visibility {
    pub fn shows(&self, storey: usize, kind: Kind) -> bool {
        self.storeys.get(storey).copied().unwrap_or(true) && (self.roof || kind != Kind::Roof)
    }
}

/// The orthographic view of a camera: rotated around z by yaw, then tilted by pitch.
#[derive(Clone, Copy)]
pub struct Projection {
    center: [f32; 3],
    /// Pixels per meter.
    pub scale: f32,
    middle: [f32; 2],
    yaw: (f32, f32),
    pitch: (f32, f32),
}

impl Projection {
    /// Fits a scene around `center` with `radius` into `width`×`height` pixels.
    pub fn new(center: [f32; 3], radius: f32, camera: Camera, width: f32, height: f32) -> Projection {
        Projection {
            center,
            scale: width.min(height) / (2.1 * radius) * camera.zoom,
            middle: [width / 2.0 + camera.pan[0], height / 2.0 + camera.pan[1]],
            yaw: camera.yaw.sin_cos(),
            pitch: camera.pitch.sin_cos(),
        }
    }

    /// Straight down onto the plan at `scale` pixels per meter, x to the right
    /// and y up as drawn, with `center` at the pixel `middle`.
    pub fn top(center: [f32; 3], scale: f32, middle: [f32; 2]) -> Projection {
        Projection { center, scale, middle, yaw: (0.0, 1.0), pitch: (1.0, 0.0) }
    }

    /// Pixel x, y and depth (larger = farther), all in pixels.
    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let c = self.center;
        let v = self.vector([p[0] - c[0], p[1] - c[1], p[2] - c[2]]);
        [self.middle[0] + v[0], self.middle[1] + v[1], v[2]]
    }

    /// A direction in pixels on screen and in depth.
    pub fn vector(&self, d: [f32; 3]) -> [f32; 3] {
        let ((sin_yaw, cos_yaw), (sin_pitch, cos_pitch)) = (self.yaw, self.pitch);
        let x1 = d[0] * cos_yaw - d[1] * sin_yaw;
        let y1 = d[0] * sin_yaw + d[1] * cos_yaw;
        let up = y1 * sin_pitch + d[2] * cos_pitch;
        let depth = y1 * cos_pitch - d[2] * sin_pitch;
        [x1 * self.scale, -up * self.scale, depth * self.scale]
    }

    /// Direction from the scene toward the camera; the opposite of the depth gradient.
    fn to_camera(self) -> [f32; 3] {
        let ((sin_yaw, cos_yaw), (sin_pitch, cos_pitch)) = (self.yaw, self.pitch);
        [-sin_yaw * cos_pitch, -cos_yaw * cos_pitch, sin_pitch]
    }
}

impl Scene {
    pub fn new(b: &Building, wall_colors: &BTreeMap<String, &'static str>) -> Scene {
        let mut faces = Vec::new();
        let mut items = Vec::new();
        for (si, s) in b.storeys.iter().enumerate() {
            for (ei, e) in s.elements.iter().enumerate() {
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
                    faces.push(Face { normal, loops, color: rgb(color), storey: si, kind: e.kind, item: items.len() as u32 });
                }
                items.push(Item { storey: si, index: ei, kind: e.kind, frame: frame(e) });
            }
        }
        Scene { items, plan_axis: plan_axis(b), ..Scene::with_faces(faces) }
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
        Scene { faces, items: Vec::new(), plan_axis: [1.0, 0.0], center, radius: radius.max(1.0), cache: Mutex::default() }
    }

    /// Paints the model with `selected` (an index from [`Scene::pick`]) highlighted,
    /// and the dimensions of that element, or of the whole model, on top.
    pub fn paint(&self, cam: Camera, vis: &Visibility, selected: Option<usize>, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let scale_factor = window.scale_factor();
        let width = (f32::from(bounds.size.width) * scale_factor).ceil() as u32;
        let height = (f32::from(bounds.size.height) * scale_factor).ceil() as u32;
        if width == 0 || height == 0 {
            return;
        }
        // Retina screens are fine as they are; standard ones get 2×2 samples per pixel.
        let supersample = if scale_factor < 1.5 { 2 } else { 1 };
        // The raster has this many pixels per logical pixel, the pan too.
        let density = scale_factor * supersample as f32;
        let raster_camera = Camera { pan: cam.pan.map(|p| p * density), ..cam };
        let view = View { camera: raster_camera, visibility: vis.clone(), selected, width, height, supersample };
        let (width, height) = (width as f32 / scale_factor, height as f32 / scale_factor);
        {
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
            cache.placement = Some((bounds.origin, density));
            let Some(image) = cache.image.clone() else { return };
            window.paint_image(Bounds::new(bounds.origin, size(px(width), px(height))), Corners::default(), image, 0, false).ok();
        }
        if !vis.dimensions {
            return;
        }
        let proj = Projection::new(self.center, self.radius, cam, width, height);
        let (frame, style, color) = match selected.filter(|&i| self.shows(i, vis)).map(|i| &self.items[i]) {
            Some(item) => (item.frame, if item.kind == Kind::Wall { Style::Wall } else { Style::Box }, SELECTED),
            None => {
                let points = self.faces.iter().filter(|f| vis.shows(f.storey, f.kind)).flat_map(|f| f.loops.iter().flatten().copied());
                (Frame::around(points, self.plan_axis), Style::Box, DIMENSION)
            }
        };
        if let Some(frame) = frame {
            let background = Rgba { a: 0.85, ..rgb(BACKGROUND) };
            let edges = style == Style::Box;
            dimension::paint(&dimension::layout(&frame, style, &proj), edges, rgb(color), background, bounds.origin, window, cx);
        }
    }

    /// The element under `position` in the last picture, as an index for
    /// [`Scene::paint`] and [`Scene::element`].
    pub fn pick(&self, position: Point<Pixels>) -> Option<usize> {
        let cache = self.cache.lock().unwrap();
        let (origin, density) = cache.placement?;
        let r = &cache.raster;
        let (x, y) = (f32::from(position.x - origin.x) * density, f32::from(position.y - origin.y) * density);
        if x < 0.0 || y < 0.0 || x >= r.width as f32 || y >= r.height as f32 {
            return None;
        }
        match r.face[y as usize * r.width + x as usize] {
            NONE => None,
            f => Some(self.faces[f as usize].item as usize),
        }
    }

    /// `cam` with the model's bounding sphere fitted into `size` less room for
    /// the dimensions, so the model keeps its size while it turns.
    pub fn room_to_turn(cam: Camera, size: Size<Pixels>) -> Camera {
        let short = f32::from(size.width).min(f32::from(size.height));
        Camera { zoom: cam.zoom * room(short) / short.max(1.0), ..cam }
    }

    /// `cam` zoomed and moved so that what `vis` shows fills `size` less room
    /// for the dimensions, for a view that does not turn; the camera's own zoom
    /// and pan apply on top.
    pub fn fit_shown(&self, cam: Camera, vis: &Visibility, size: Size<Pixels>) -> Camera {
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        let unit = Projection::new(self.center, self.radius, Camera { zoom: 1.0, pan: [0.0, 0.0], ..cam }, w, h);
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in self.faces.iter().filter(|f| vis.shows(f.storey, f.kind)).flat_map(|f| f.loops.iter().flatten()) {
            let q = unit.point(*p);
            for k in 0..2 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        if lo[0] > hi[0] {
            return cam;
        }
        let k = (room(w) / (hi[0] - lo[0]).max(1.0)).min(room(h) / (hi[1] - lo[1]).max(1.0));
        // How far the middle of what is shown lies off the view's middle, at the fitted size.
        let off = [((lo[0] + hi[0]) / 2.0 - w / 2.0) * k, ((lo[1] + hi[1]) / 2.0 - h / 2.0) * k];
        Camera { zoom: cam.zoom * k, pan: [cam.pan[0] - off[0] * cam.zoom, cam.pan[1] - off[1] * cam.zoom], ..cam }
    }

    /// The yaw that looks straight at one side of the building: 0 from the
    /// south, then east, north and west, the plan's y axis being north.
    pub fn facade(&self, side: usize) -> f32 {
        -self.plan_axis[1].atan2(self.plan_axis[0]) - side as f32 * std::f32::consts::FRAC_PI_2
    }

    /// Storey and position in it of a picked element.
    pub fn element(&self, item: usize) -> (usize, usize) {
        (self.items[item].storey, self.items[item].index)
    }

    /// Whether `vis` shows the picked element `item`.
    pub fn shows(&self, item: usize, vis: &Visibility) -> bool {
        self.items.get(item).is_some_and(|i| vis.shows(i.storey, i.kind))
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
        let proj = Projection::new(self.center, self.radius, cam, w as f32, h as f32);
        let to_camera = proj.to_camera();

        // Back faces of the closed solids are always hidden; skipping them halves the work.
        let visible: Vec<(u32, Vec<Vec<[f32; 3]>>)> = self
            .faces
            .iter()
            .enumerate()
            .filter(|(_, f)| view.visibility.shows(f.storey, f.kind))
            .filter(|(_, f)| dot(f.normal, to_camera) > -0.02)
            .map(|(i, f)| (i as u32, f.loops.iter().map(|l| l.iter().map(|p| proj.point(*p)).collect()).collect()))
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

    /// The finished picture: shaded faces with darker outlines, the selected
    /// element tinted, averaged down to the view's size, as GPUI's BGRA with
    /// straight alpha. Empty pixels stay transparent.
    fn resolve(&self, faces: &[Face], view: &View) -> Vec<u8> {
        let light = normalize([0.35, -0.55, 0.75]);
        let selected = view.selected.map(|i| i as u32);
        let color: Vec<Rgba> = faces.iter().map(|f| if Some(f.item) == selected { mix(f.color, rgb(SELECTED), 0.55) } else { f.color }).collect();
        let fill: Vec<[f32; 3]> = faces
            .iter()
            .zip(&color)
            .map(|(f, c)| {
                let shade = 0.6 + 0.4 * dot(f.normal, light).max(0.0);
                [c.r * shade, c.g * shade, c.b * shade]
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
                            let line = color[self.edge[i] as usize];
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

/// The box an element's dimensions are taken from. A wall's is that of its
/// reference face and thickness, so the values match its quantities.
pub fn frame(e: &Element) -> Option<Frame> {
    let f = |v: &V3| [v.x as f32, v.y as f32, v.z as f32];
    let (axis, normal) = (f(&e.axis), f(&e.normal));
    if e.kind != Kind::Wall {
        let l = (axis[0] * axis[0] + axis[1] * axis[1]).sqrt();
        if l < 1e-6 {
            return None;
        }
        return Frame::around(e.faces.iter().flatten().flatten().map(f), [axis[0] / l, axis[1] / l]);
    }
    let (lo, hi) = e.outline.iter().map(|p| dot(f(p), axis)).fold((f32::MAX, f32::MIN), |(lo, hi), s| (lo.min(s), hi.max(s)));
    // Axis and normal are horizontal.
    let w = dot(f(e.outline.first()?), normal);
    let origin = [axis[0] * lo + normal[0] * w, axis[1] * lo + normal[1] * w, e.bottom_z as f32];
    Some(Frame { origin, axes: [axis, normal], size: [hi - lo, e.thickness as f32, e.height as f32] })
}

/// The direction most elements run along in plan, weighted by length and
/// folded to a quarter turn: the building's own axes, also in a rotated drawing.
pub fn plan_axis(b: &Building) -> [f32; 2] {
    let (mut c, mut s) = (0.0, 0.0);
    for e in b.storeys.iter().flat_map(|s| &s.elements) {
        let angle = 4.0 * e.axis.y.atan2(e.axis.x);
        (c, s) = (c + e.length * angle.cos(), s + e.length * angle.sin());
    }
    let angle = (s.atan2(c) / 4.0) as f32;
    [angle.cos(), angle.sin()]
}

/// What is left of `side` pixels for the model after the room for its dimensions.
fn room(side: f32) -> f32 {
    (side - 2.0 * ROOM).max(0.2 * side)
}

pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1.0 }
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
    use gpui::point;

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
        Face { loops, normal, color: rgb(color), storey: 0, kind: Kind::Wall, item: 0 }
    }

    fn view() -> View {
        let camera = Camera { yaw: 0.0, pitch: 0.0, zoom: 1.0, pan: [0.0, 0.0] };
        let visibility = Visibility { storeys: vec![true], roof: true, dimensions: true };
        View { camera, visibility, selected: None, width: 100, height: 100, supersample: 1 }
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
    fn picks_the_element_in_front() {
        let mut back = square(0.0, 1.0, false, 0x0000ff);
        back.item = 1;
        let scene = Scene::with_faces(vec![back, square(-1.0, 0.5, false, 0xff0000)]);
        {
            let mut cache = scene.cache.lock().unwrap();
            scene.rasterize(&view(), &mut cache.raster);
            cache.placement = Some((point(px(10.0), px(20.0)), 1.0));
        }
        // The picture is painted at (10, 20); the small square covers its middle.
        assert_eq!(scene.pick(point(px(60.0), px(70.0))), Some(0));
        assert_eq!(scene.pick(point(px(35.0), px(70.0))), Some(1));
        assert_eq!(scene.pick(point(px(15.0), px(70.0))), None);
        assert_eq!(scene.pick(point(px(5.0), px(70.0))), None);
    }

    #[test]
    fn facades_are_seen_square_on() {
        let scene = Scene { plan_axis: [1.0, 0.0], ..Scene::with_faces(vec![]) };
        let seen = |side: usize, p: [f32; 3]| {
            let camera = Camera { yaw: scene.facade(side), pitch: 0.0, zoom: 1.0, pan: [0.0, 0.0] };
            let q = Projection::new([0.0; 3], 1.0, camera, 210.0, 210.0).point(p);
            [q[0] - 105.0, q[1] - 105.0, q[2]].map(|v| (v * 10.0).round() / 10.0)
        };
        // From the south east is to the right; from the east north is, and east is nearest.
        assert_eq!(seen(0, [1.0, 0.0, 0.0]), [100.0, 0.0, 0.0]);
        assert_eq!(seen(1, [0.0, 1.0, 0.0]), [100.0, 0.0, 0.0]);
        assert_eq!(seen(1, [1.0, 0.0, 0.0])[2], -100.0);
        assert_eq!(seen(2, [-1.0, 0.0, 1.0]), [100.0, -100.0, 0.0]);
        assert_eq!(seen(3, [0.0, -1.0, 0.0]), [100.0, 0.0, 0.0]);
    }

    #[test]
    fn shown_part_fills_the_view_less_room() {
        // A 2 m square in the middle, and one 6 m to its right on a hidden storey.
        let mut far = square(0.0, 1.0, false, 0x0000ff);
        far.loops[0].iter_mut().for_each(|p| p[0] += 6.0);
        far.storey = 1;
        let scene = Scene::with_faces(vec![square(0.0, 1.0, false, 0xff0000), far]);
        let vis = Visibility { storeys: vec![true, false], roof: true, dimensions: true };
        let camera = scene.fit_shown(view().camera, &vis, size(px(500.0), px(400.0)));
        let proj = Projection::new(scene.center, scene.radius, camera, 500.0, 400.0);
        // 400 px high less 100 px room above and below; centered.
        let round = |p: [f32; 3]| [p[0].round(), p[1].round()];
        assert_eq!(round(proj.point([-1.0, 0.0, 1.0])), [150.0, 100.0]);
        assert_eq!(round(proj.point([1.0, 0.0, -1.0])), [350.0, 300.0]);
    }

    #[test]
    fn zooming_keeps_the_middle_in_place() {
        let mut view = PanZoom { pan: [40.0, -20.0], zoom: 1.0 };
        view.zoom_by(2.0);
        assert_eq!((view.pan, view.zoom), ([80.0, -40.0], 2.0));
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
