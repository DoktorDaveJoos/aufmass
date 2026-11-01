//! Software 3D preview: flat-shaded faces drawn back to front (painter's
//! algorithm) with GPUI paths. Holes (windows, doors) are separate loops of a
//! face and stay open through even-odd filling.

use aufmass::model::{Building, Kind};
use gpui::{point, px, rgb, Bounds, PathBuilder, Pixels, Rgba, Window};
use std::collections::BTreeMap;

pub const SLAB_COLOR: u32 = 0x9fb4c7;
pub const ROOF_COLOR: u32 = 0xb5654f;

struct Face {
    loops: Vec<Vec<[f32; 3]>>,
    normal: [f32; 3],
    /// Center of the outer loop; orders faces within one element.
    centroid: [f32; 3],
    /// Index into `Scene::element_centers`.
    element: usize,
    color: Rgba,
    storey: usize,
    kind: Kind,
}

pub struct Scene {
    faces: Vec<Face>,
    /// Bounding-box centers of the elements. Elements are drawn back to front
    /// by these: layered walls (core, cladding) keep their order even when the
    /// layers differ in width, which single-face sorting gets wrong.
    element_centers: Vec<[f32; 3]>,
    center: [f32; 3],
    radius: f32,
}

#[derive(Clone, Copy)]
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
#[derive(Clone)]
pub struct Visibility {
    pub storeys: Vec<bool>,
    pub roof: bool,
}

impl Scene {
    pub fn new(b: &Building, wall_colors: &BTreeMap<String, &'static str>) -> Scene {
        let mut faces = Vec::new();
        let mut element_centers = Vec::new();
        for (si, s) in b.storeys.iter().enumerate() {
            for e in &s.elements {
                let element = element_centers.len();
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for p in e.faces.iter().flatten().flatten() {
                    for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                        lo[k] = lo[k].min(v as f32);
                        hi[k] = hi[k].max(v as f32);
                    }
                }
                element_centers.push([0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0));
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
                    let n = outer.len() as f32;
                    let centroid = [0, 1, 2].map(|k| outer.iter().map(|p| p[k]).sum::<f32>() / n);
                    let normal = newell(outer).0;
                    faces.push(Face { normal, centroid, element, loops, color: rgb(color), storey: si, kind: e.kind });
                }
            }
        }
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in faces.iter().flat_map(|f| f.loops.iter().flatten()) {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let radius = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() / 2.0;
        Scene { faces, element_centers, center, radius: radius.max(1.0) }
    }

    pub fn paint(&self, cam: Camera, vis: &Visibility, bounds: Bounds<Pixels>, window: &mut Window) {
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (ox, oy) = (f32::from(bounds.origin.x) + w / 2.0, f32::from(bounds.origin.y) + h / 2.0);
        let scale = w.min(h) / (2.1 * self.radius) * cam.zoom;
        let (sin_yaw, cos_yaw) = cam.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = cam.pitch.sin_cos();
        let c = self.center;
        // Rotate around z by yaw, then tilt by pitch; returns screen x, y and depth (larger = farther).
        let project = |p: &[f32; 3]| {
            let (x, y, z) = (p[0] - c[0], p[1] - c[1], p[2] - c[2]);
            let x1 = x * cos_yaw - y * sin_yaw;
            let y1 = x * sin_yaw + y * cos_yaw;
            let up = y1 * sin_pitch + z * cos_pitch;
            let depth = y1 * cos_pitch - z * sin_pitch;
            (ox + x1 * scale, oy - up * scale, depth)
        };
        let light = normalize([0.35, -0.55, 0.75]);
        // Direction from the scene toward the camera; the opposite of the depth gradient.
        let to_camera = [-sin_yaw * cos_pitch, -cos_yaw * cos_pitch, sin_pitch];

        // Back faces of the closed solids are never visible; skipping them keeps
        // the inner face of a cladding layer from covering the wall behind it.
        let element_depth: Vec<f32> = self.element_centers.iter().map(|c| project(c).2).collect();
        let mut order: Vec<(f32, f32, usize)> = self
            .faces
            .iter()
            .enumerate()
            .filter(|(_, f)| vis.storeys.get(f.storey).copied().unwrap_or(true) && (vis.roof || f.kind != Kind::Roof))
            .filter(|(_, f)| dot(f.normal, to_camera) > -0.02)
            .map(|(i, f)| (element_depth[f.element], project(&f.centroid).2, i))
            .collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.total_cmp(&a.1)));

        for (_, _, i) in order {
            let f = &self.faces[i];
            let shade = 0.6 + 0.4 * dot(f.normal, light).max(0.0);
            let mut fill = PathBuilder::fill();
            let mut edge = PathBuilder::stroke(px(0.6));
            for l in &f.loops {
                for (k, p) in l.iter().enumerate() {
                    let (x, y, _) = project(p);
                    let pt = point(px(x), px(y));
                    if k == 0 {
                        fill.move_to(pt);
                        edge.move_to(pt);
                    } else {
                        fill.line_to(pt);
                        edge.line_to(pt);
                    }
                }
                fill.close();
                edge.close();
            }
            let base = f.color;
            if let Ok(path) = fill.build() {
                window.paint_path(path, Rgba { r: base.r * shade, g: base.g * shade, b: base.b * shade, a: 1.0 });
            }
            if let Ok(path) = edge.build() {
                window.paint_path(path, Rgba { r: base.r * 0.35, g: base.g * 0.35, b: base.b * 0.35, a: 0.55 });
            }
        }
    }
}

/// "#c0601e" -> 0xc0601e
pub fn hex(color: &str) -> u32 {
    u32::from_str_radix(color.trim_start_matches('#'), 16).unwrap_or(0x888888)
}

/// Unit normal and twice the area of a planar loop.
fn newell(pts: &[[f32; 3]]) -> ([f32; 3], f32) {
    let mut n = [0.0f32; 3];
    for (i, a) in pts.iter().enumerate() {
        let b = pts[(i + 1) % pts.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    (normalize(n), dot(n, n).sqrt())
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt();
    if l > 0.0 { [v[0] / l, v[1] / l, v[2] / l] } else { v }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
