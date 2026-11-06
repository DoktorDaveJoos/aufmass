//! The model seen from above: one storey as a floor plan, drawn as vectors in
//! the drawing's own orientation, with the same dimensions as the 3D preview.
//! Walls are filled in their colors with their openings across them, as in the
//! plan previews; slabs are tinted underneath and roofs dashed on top.

use crate::dimension::{self, Frame, Style};
use crate::scene::{self, PanZoom, Projection, Visibility};
use aufmass::classify::wall_footprint;
use aufmass::model::{Building, Kind, OpeningKind};
use gpui::{point, px, rgb, App, Bounds, PathBuilder, Pixels, Point, Rgba, Window};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Room around the plan for its dimensions, in pixels.
const MARGIN: f32 = 72.0;
/// How far from a wall a click still picks it, in pixels; cladding is only a pixel or two wide.
const REACH: f32 = 3.0;

/// One element in plan.
struct Shape {
    /// Index of the element as in [`scene::Scene`]: counted through the storeys.
    item: usize,
    storey: usize,
    kind: Kind,
    /// Outline, then holes.
    loops: Vec<Vec<[f32; 2]>>,
    color: u32,
    /// Openings across a wall, from face to face.
    openings: Vec<([[f32; 2]; 4], OpeningKind)>,
    frame: Option<Frame>,
}

pub struct FloorPlan {
    /// In drawing order: slabs, walls, roofs, the larger of a kind first.
    shapes: Vec<Shape>,
    plan_axis: [f32; 2],
    /// Middle and half the size of the whole building in plan, so the storeys line up.
    center: [f32; 2],
    half: [f32; 2],
    /// The last picture, for picking.
    last: Mutex<Option<(Placement, Visibility)>>,
}

/// Where the plan was painted and at what scale.
#[derive(Clone, Copy)]
struct Placement {
    origin: Point<Pixels>,
    center: [f32; 2],
    scale: f32,
    /// The pixel `center` is at, from `origin`.
    middle: [f32; 2],
}

impl Placement {
    fn projection(&self) -> Projection {
        Projection::top([self.center[0], self.center[1], 0.0], self.scale, self.middle)
    }

    /// The plan point under a window position.
    fn plan(&self, p: Point<Pixels>) -> [f32; 2] {
        let (x, y) = (f32::from(p.x - self.origin.x), f32::from(p.y - self.origin.y));
        [self.center[0] + (x - self.middle[0]) / self.scale, self.center[1] - (y - self.middle[1]) / self.scale]
    }
}

impl FloorPlan {
    pub fn new(b: &Building, wall_colors: &BTreeMap<String, &'static str>) -> FloorPlan {
        let elements = b.storeys.iter().enumerate().flat_map(|(si, s)| s.elements.iter().map(move |e| (si, e)));
        let shapes = elements
            .enumerate()
            .filter_map(|(item, (storey, e))| {
                let xy = |p: (f64, f64)| [p.0 as f32, p.1 as f32];
                let (loops, color, openings) = match e.kind {
                    Kind::Wall => {
                        let footprint = wall_footprint(e)?;
                        let (start, half) = (e.start?, e.normal * (e.thickness / 2.0 + 0.02));
                        let openings = e
                            .openings
                            .iter()
                            .map(|o| {
                                let (a, b) = (start + e.axis * o.offset, start + e.axis * (o.offset + o.width));
                                ([(a - half).xy(), (b - half).xy(), (b + half).xy(), (a + half).xy()].map(xy), o.kind)
                            })
                            .collect();
                        let color = wall_colors.get(&e.name).map_or(0x888888, |c| scene::hex(c));
                        (vec![footprint.map(xy).to_vec()], color, openings)
                    }
                    Kind::Slab | Kind::Roof => {
                        let loops = std::iter::once(&e.outline).chain(&e.holes).map(|l| l.iter().map(|p| xy(p.xy())).collect()).collect();
                        (loops, if e.kind == Kind::Slab { scene::SLAB_COLOR } else { scene::ROOF_COLOR }, Vec::new())
                    }
                };
                Some(Shape { item, storey, kind: e.kind, loops, color, openings, frame: scene::frame(e) })
            })
            .collect();
        FloorPlan { plan_axis: scene::plan_axis(b), ..FloorPlan::with_shapes(shapes) }
    }

    fn with_shapes(mut shapes: Vec<Shape>) -> FloorPlan {
        let order = |k: Kind| match k {
            Kind::Slab => 0,
            Kind::Wall => 1,
            Kind::Roof => 2,
        };
        shapes.sort_by(|a, b| order(a.kind).cmp(&order(b.kind)).then(area(&b.loops).total_cmp(&area(&a.loops))));
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in shapes.iter().flat_map(|s| &s.loops[0]) {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let half = [((hi[0] - lo[0]) / 2.0).max(1.0), ((hi[1] - lo[1]) / 2.0).max(1.0)];
        FloorPlan { shapes, plan_axis: [1.0, 0.0], center, half, last: Mutex::default() }
    }

    /// Paints the storeys `vis` shows with `selected` (an item as in [`scene::Scene`])
    /// highlighted, and the dimensions of that element, or of everything shown, on top.
    pub fn paint(&self, cam: PanZoom, vis: &Visibility, selected: Option<usize>, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let fit = ((w - 2.0 * MARGIN) / (2.0 * self.half[0])).min((h - 2.0 * MARGIN) / (2.0 * self.half[1])).max(1.0);
        let place = Placement { origin: bounds.origin, center: self.center, scale: fit * cam.zoom, middle: [w / 2.0 + cam.pan[0], h / 2.0 + cam.pan[1]] };
        let proj = place.projection();
        let at = |p: [f32; 2]| {
            let s = proj.point([p[0], p[1], 0.0]);
            bounds.origin + point(px(s[0]), px(s[1]))
        };
        let shown: Vec<&Shape> = self.shapes.iter().filter(|s| vis.shows(s.storey, s.kind)).collect();
        for s in &shown {
            paint_shape(s, Some(s.item) == selected, &at, window);
        }
        *self.last.lock().unwrap() = Some((place, vis.clone()));
        if !vis.dimensions {
            return;
        }
        let (frame, style, color) = match selected.and_then(|i| shown.iter().find(|s| s.item == i)) {
            Some(s) => (s.frame, if s.kind == Kind::Wall { Style::Wall } else { Style::Box }, scene::SELECTED),
            None => {
                let points = shown.iter().flat_map(|s| s.loops[0].iter().map(|p| [p[0], p[1], 0.0]));
                (Frame::around(points, self.plan_axis), Style::Box, scene::DIMENSION)
            }
        };
        if let Some(frame) = frame {
            let background = Rgba { a: 0.85, ..rgb(scene::BACKGROUND) };
            dimension::paint(&dimension::layout(&frame, style, &proj), style == Style::Box, rgb(color), background, bounds.origin, window, cx);
        }
    }

    /// The element under `position` in the last picture: a wall before the
    /// slab it stands on, the smallest of overlapping slabs, a roof last.
    pub fn pick(&self, position: Point<Pixels>) -> Option<usize> {
        let last = self.last.lock().unwrap();
        let (place, vis) = last.as_ref()?;
        let p = place.plan(position);
        let shown = || self.shapes.iter().rev().filter(|s| vis.shows(s.storey, s.kind));
        let near = REACH / place.scale;
        shown()
            .find(|s| s.kind == Kind::Wall && inside(&s.loops, p))
            .or_else(|| shown().find(|s| s.kind == Kind::Wall && distance(&s.loops, p) < near))
            .or_else(|| shown().find(|s| s.kind == Kind::Slab && inside(&s.loops, p)))
            .or_else(|| shown().find(|s| s.kind == Kind::Roof && inside(&s.loops, p)))
            .map(|s| s.item)
    }
}

fn paint_shape(s: &Shape, selected: bool, at: &impl Fn([f32; 2]) -> Point<Pixels>, window: &mut Window) {
    let outline = |builder: &mut PathBuilder, loops: &[Vec<[f32; 2]>]| {
        for l in loops {
            builder.add_polygon(&l.iter().map(|&p| at(p)).collect::<Vec<_>>(), true);
        }
    };
    let accent = rgb(scene::SELECTED);
    let color = rgb(s.color);
    let (fill, line) = match s.kind {
        // Slabs lie underneath, so they stay light.
        Kind::Slab => (scene::mix(color, rgb(scene::BACKGROUND), 0.6), scene::mix(color, rgb(0x000000), 0.3)),
        Kind::Wall => (color, scene::mix(color, rgb(0x000000), 0.6)),
        // Roofs cover the storey, so only their outline is drawn.
        Kind::Roof => (Rgba { a: 0.0, ..color }, color),
    };
    let (fill, line) = match (selected, s.kind) {
        (false, _) => (fill, line),
        (true, Kind::Roof) => (Rgba { a: 0.08, ..accent }, accent),
        (true, _) => (scene::mix(fill, accent, 0.55), accent),
    };
    if fill.a > 0.0 {
        let mut area = PathBuilder::fill();
        outline(&mut area, &s.loops);
        if let Ok(path) = area.build() {
            window.paint_path(path, fill);
        }
    }
    let width = px(if selected { 1.5 } else { 1.0 });
    let mut edges = if s.kind == Kind::Roof { PathBuilder::stroke(width).dash_array(&[px(6.0), px(4.0)]) } else { PathBuilder::stroke(width) };
    outline(&mut edges, &s.loops);
    if let Ok(path) = edges.build() {
        window.paint_path(path, line);
    }
    for (rect, kind) in &s.openings {
        let points: Vec<Point<Pixels>> = rect.iter().map(|&p| at(p)).collect();
        let mut gap = PathBuilder::fill();
        gap.add_polygon(&points, true);
        let mut frame = PathBuilder::stroke(px(1.2));
        frame.add_polygon(&points, true);
        if let (Ok(gap), Ok(frame)) = (gap.build(), frame.build()) {
            window.paint_path(gap, rgb(scene::BACKGROUND));
            window.paint_path(frame, rgb(opening_color(*kind)));
        }
    }
}

/// Doors red, windows blue, as in the plan previews.
pub fn opening_color(kind: OpeningKind) -> u32 {
    match kind {
        OpeningKind::Door => 0xc0392b,
        OpeningKind::Window => 0x2471a3,
        OpeningKind::Other => 0xe08e0b,
    }
}

/// Whether `p` lies inside the outline and outside its holes.
fn inside(loops: &[Vec<[f32; 2]>], p: [f32; 2]) -> bool {
    let mut odd = false;
    for l in loops {
        for (k, a) in l.iter().enumerate() {
            let b = l[(k + 1) % l.len()];
            if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
                odd = !odd;
            }
        }
    }
    odd
}

/// Distance from `p` to the nearest edge of the loops.
fn distance(loops: &[Vec<[f32; 2]>], p: [f32; 2]) -> f32 {
    let mut best = f32::MAX;
    for l in loops {
        for (k, a) in l.iter().enumerate() {
            let b = l[(k + 1) % l.len()];
            let (d, v) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
            let length = d[0] * d[0] + d[1] * d[1];
            let t = if length > 0.0 { ((v[0] * d[0] + v[1] * d[1]) / length).clamp(0.0, 1.0) } else { 0.0 };
            best = best.min((v[0] - d[0] * t).hypot(v[1] - d[1] * t));
        }
    }
    best
}

/// Area of the outline less its holes.
fn area(loops: &[Vec<[f32; 2]>]) -> f32 {
    let signed = |l: &Vec<[f32; 2]>| (0..l.len()).map(|k| l[k][0] * l[(k + 1) % l.len()][1] - l[(k + 1) % l.len()][0] * l[k][1]).sum::<f32>() / 2.0;
    loops.first().map_or(0.0, |o| signed(o).abs()) - loops.iter().skip(1).map(|h| signed(h).abs()).sum::<f32>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    fn shape(item: usize, kind: Kind, loops: Vec<Vec<[f32; 2]>>) -> Shape {
        Shape { item, storey: 0, kind, loops, color: 0x888888, openings: Vec::new(), frame: None }
    }

    /// A 10 × 8 m storey: a slab with a stairwell, a small slab on it, a wall
    /// along its south edge, 2 cm cladding along its east edge and the roof.
    fn plan() -> FloorPlan {
        let plan = FloorPlan::with_shapes(vec![
            shape(0, Kind::Roof, vec![rect(-1.0, -1.0, 11.0, 9.0)]),
            shape(1, Kind::Wall, vec![rect(0.0, 0.0, 10.0, 0.3)]),
            shape(2, Kind::Wall, vec![rect(10.0, 0.0, 10.02, 8.0)]),
            shape(3, Kind::Slab, vec![rect(0.0, 0.0, 10.0, 8.0), rect(4.0, 4.0, 6.0, 6.0)]),
            shape(4, Kind::Slab, vec![rect(1.0, 5.0, 3.0, 7.0)]),
        ]);
        // 10 px per meter, the plan's middle (5, 4) at (500, 400) in a canvas at (100, 50).
        let place = Placement { origin: point(px(100.0), px(50.0)), center: plan.center, scale: 10.0, middle: [500.0, 400.0] };
        let vis = Visibility { storeys: vec![true], roof: true, dimensions: true };
        *plan.last.lock().unwrap() = Some((place, vis));
        plan
    }

    /// The window position of a plan point in [`plan`].
    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(100.0 + 500.0 + (x - 5.0) * 10.0), px(50.0 + 400.0 - (y - 4.0) * 10.0))
    }

    #[test]
    fn draws_slabs_under_walls_under_the_roof() {
        let kinds: Vec<(Kind, usize)> = plan().shapes.iter().map(|s| (s.kind, s.item)).collect();
        assert_eq!(kinds, [(Kind::Slab, 3), (Kind::Slab, 4), (Kind::Wall, 1), (Kind::Wall, 2), (Kind::Roof, 0)]);
    }

    #[test]
    fn picks_what_lies_on_top() {
        let plan = plan();
        assert_eq!(plan.pick(at(5.0, 0.15)), Some(1), "the wall, not the slab under it");
        assert_eq!(plan.pick(at(2.0, 6.0)), Some(4), "the small slab on the large one");
        assert_eq!(plan.pick(at(2.0, 2.0)), Some(3));
        assert_eq!(plan.pick(at(5.0, 5.0)), Some(0), "through the stairwell to the roof");
        assert_eq!(plan.pick(at(10.5, 4.0)), Some(0), "the overhang");
        assert_eq!(plan.pick(at(12.0, 4.0)), None);
    }

    #[test]
    fn thin_walls_can_be_hit_beside_them() {
        let plan = plan();
        assert_eq!(plan.pick(at(10.2, 4.0)), Some(2), "2 px off the cladding");
        assert_eq!(plan.pick(at(10.5, 4.0)), Some(0), "5 px off it is the roof");
    }

    #[test]
    fn plan_has_x_to_the_right_and_y_up() {
        let proj = Projection::top([5.0, 4.0, 0.0], 10.0, [500.0, 400.0]);
        assert_eq!(proj.point([6.0, 5.0, 0.0])[..2], [510.0, 390.0]);
    }
}
