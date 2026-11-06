//! Technical dimensions over the 3D preview, drawn as in German building
//! drawings (DIN 406, DIN 1356): extension lines from the measured edges, a
//! dimension line with oblique ticks, and the value in meters from 1 m and in
//! centimeters below, with the millimeters raised.

use crate::scene::Projection;
use gpui::{fill, point, px, size, App, Bounds, PathBuilder, Pixels, Point, Rgba, TextRun, Window};

// Screen distances in logical pixels.
/// Between a measured point and the start of its extension line.
const GAP: f32 = 3.0;
/// From the measured edge to the dimension line.
const OFFSET: f32 = 22.0;
/// Of the extension lines past the dimension line.
const OVERSHOOT: f32 = 4.0;
/// Of the dimension line past the extension lines.
const RUN_ON: f32 = 3.0;
/// Half the length of an oblique tick.
const TICK: f32 = 5.5;
/// Dimensions shorter than this on screen are seen end-on and left out.
const MIN_LENGTH: f32 = 6.0;
const FONT: f32 = 12.0;

const UP: [f32; 3] = [0.0, 0.0, 1.0];
const DOWN: [f32; 3] = [0.0, 0.0, -1.0];

/// An oriented box: `origin + s·axes[0] + t·axes[1] + z·UP` for `[s, t, z]` within `size`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: [f32; 3],
    /// Horizontal unit vectors at a right angle.
    pub axes: [[f32; 3]; 2],
    pub size: [f32; 3],
}

impl Frame {
    /// The box around `points` with its first side along the plan direction `u`, a unit vector.
    pub fn around(points: impl IntoIterator<Item = [f32; 3]>, u: [f32; 2]) -> Option<Frame> {
        let axes = [[u[0], u[1], 0.0], [-u[1], u[0], 0.0]];
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in points {
            let c = [dot(p, axes[0]), dot(p, axes[1]), p[2]];
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        let origin = add(add(mul(axes[0], lo[0]), mul(axes[1], lo[1])), mul(UP, lo[2]));
        (lo[0] <= hi[0]).then(|| Frame { origin, axes, size: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]] })
    }

    /// The point at box coordinates `[s, t, z]`.
    fn at(&self, c: [f32; 3]) -> [f32; 3] {
        add(add(self.origin, mul(self.axes[0], c[0])), add(mul(self.axes[1], c[1]), mul(UP, c[2])))
    }
}

/// How a box is dimensioned.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Style {
    /// Both sides in plan along the bottom edges nearest the camera, the height at the right.
    Box,
    /// A wall, `size` being length, thickness and height: the length along the
    /// bottom, the height at the right end and the thickness across its top.
    Wall,
}

/// One dimension on screen, in pixels from the canvas origin.
#[derive(Debug)]
pub struct Dim {
    /// The measured edge.
    edge: [[f32; 2]; 2],
    /// Where the extension lines start, just off the measured points.
    from: [[f32; 2]; 2],
    /// Where they cross the dimension line.
    to: [[f32; 2]; 2],
    /// Unit direction from the object toward the dimension line.
    out: [f32; 2],
    value: f32,
}

/// The dimensions of `frame` placed for the view `proj`. Each runs along an
/// edge facing the camera and stands off the box in whichever of a few
/// directions is seen least foreshortened.
pub fn layout(frame: &Frame, style: Style, proj: &Projection) -> Vec<Dim> {
    let [ls, lt, lz] = frame.size;
    let screen = |c: [f32; 3]| proj.point(frame.at(c));
    let center = screen([ls / 2.0, lt / 2.0, lz / 2.0]);
    // Away from the box along side `k`, at box coordinate `at` on it.
    let out = |k: usize, at: f32| if at > 0.0 { frame.axes[k] } else { mul(frame.axes[k], -1.0) };
    let mut dims = Vec::new();
    let mut add = |a: [f32; 3], b: [f32; 3], value: f32, choices: &[([f32; 3], f32)]| {
        dims.extend(place(proj, screen(a), screen(b), value, choices, center));
    };

    // The height at the vertical edge farthest right on screen, the nearer of two in line.
    let key = |c: &[f32; 2]| {
        let p = screen([c[0], c[1], 0.0]);
        p[0] - 0.01 * p[2]
    };
    let corner = [[0.0, 0.0], [ls, 0.0], [0.0, lt], [ls, lt]].into_iter().max_by(|a, b| key(a).total_cmp(&key(b))).unwrap();
    add([corner[0], corner[1], 0.0], [corner[0], corner[1], lz], lz, &[(out(0, corner[0]), 1.0), (out(1, corner[1]), 1.0)]);

    for k in 0..2 {
        let j = 1 - k;
        // The edge along side `k` at coordinate `at` of side `j`, at height `z`.
        let edge = |at: f32, z: f32| {
            let mut a = [0.0, 0.0, z];
            a[j] = at;
            let mut b = a;
            b[k] = frame.size[k];
            (a, b)
        };
        if style == Style::Wall && k == 1 {
            let (a, b) = edge(corner[0], lz);
            add(a, b, lt, &[(UP, 1.0), (out(0, corner[0]), 0.6)]);
        } else {
            let depth = |at: f32| {
                let (a, b) = edge(at, 0.0);
                screen(a)[2] + screen(b)[2]
            };
            let at = if depth(0.0) <= depth(frame.size[j]) { 0.0 } else { frame.size[j] };
            let (a, b) = edge(at, 0.0);
            add(a, b, frame.size[k], &[(out(j, at), 1.0), (DOWN, 0.6)]);
        }
    }
    dims
}

/// A dimension of `value` between the screen points `a` and `b`, standing off
/// in the best of `choices` (a direction and how much it is preferred) that
/// leads away from the box `center`; none when all of them are seen end-on.
fn place(proj: &Projection, a: [f32; 3], b: [f32; 3], value: f32, choices: &[([f32; 3], f32)], center: [f32; 3]) -> Option<Dim> {
    let (a, b) = ([a[0], a[1]], [b[0], b[1]]);
    let length = len2(sub2(b, a));
    if length < MIN_LENGTH {
        return None;
    }
    let along = mul2(sub2(b, a), 1.0 / length);
    let away = sub2(mul2(add2(a, b), 0.5), [center[0], center[1]]);
    let (out, sin) = choices
        .iter()
        .filter_map(|&(dir, preference)| {
            let v = proj.vector(dir);
            let v = [v[0], v[1]];
            let seen = len2(v);
            // The part of the direction that leads off the line, as seen.
            let off = cross2(along, v).abs() / proj.scale;
            (seen > 0.0 && dot2(v, away) >= 0.0).then(|| (mul2(v, 1.0 / seen), off * proj.scale / seen, off * preference))
        })
        .filter(|c| c.2 > 0.15)
        .max_by(|x, y| x.2.total_cmp(&y.2))
        .map(|(out, sin, _)| (out, sin))?;
    // The dimension line keeps its distance from the edge, however oblique the extension lines.
    let reach = |d: f32| mul2(out, d / sin);
    Some(Dim { edge: [a, b], from: [add2(a, reach(GAP)), add2(b, reach(GAP))], to: [add2(a, reach(OFFSET)), add2(b, reach(OFFSET))], out, value })
}

/// Paints `dims` in `color` over the canvas at `origin`; the values sit on
/// `background`. With `edges`, the measured edges are dashed, for a box that
/// is not itself drawn.
pub fn paint(dims: &[Dim], edges: bool, color: Rgba, background: Rgba, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
    if dims.is_empty() {
        return;
    }
    let at = |p: [f32; 2]| origin + point(px(p[0]), px(p[1]));
    if edges {
        let mut dashes = PathBuilder::stroke(px(1.0)).dash_array(&[px(4.0), px(3.0)]);
        for d in dims {
            dashes.move_to(at(d.edge[0]));
            dashes.line_to(at(d.edge[1]));
        }
        if let Ok(path) = dashes.build() {
            window.paint_path(path, Rgba { a: 0.45, ..color });
        }
    }
    let mut lines = PathBuilder::stroke(px(1.0));
    let mut ticks = PathBuilder::stroke(px(2.0));
    for d in dims {
        let along = unit2(sub2(d.to[1], d.to[0]));
        for k in 0..2 {
            lines.move_to(at(d.from[k]));
            lines.line_to(at(add2(d.to[k], mul2(d.out, OVERSHOOT))));
        }
        lines.move_to(at(sub2(d.to[0], mul2(along, RUN_ON))));
        lines.line_to(at(add2(d.to[1], mul2(along, RUN_ON))));
        let tick = mul2(oblique(along), TICK);
        for p in d.to {
            ticks.move_to(at(sub2(p, tick)));
            ticks.line_to(at(add2(p, tick)));
        }
    }
    for path in [lines, ticks] {
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
    for d in dims {
        label(d, color, background, &at, window, cx);
    }
}

/// The value next to the middle of the dimension line, on the side away from
/// the object, or past its end when the line is too short to hold it.
fn label(d: &Dim, color: Rgba, background: Rgba, at: &impl Fn([f32; 2]) -> Point<Pixels>, window: &mut Window, cx: &mut App) {
    let Some((value, raised)) = din(d.value) else { return };
    let text_system = window.text_system().clone();
    let font = window.text_style().font();
    let shape = |text: String, size: f32| {
        let run = TextRun { len: text.len(), font: font.clone(), color: color.into(), background_color: None, underline: None, strikethrough: None };
        text_system.shape_line(text.into(), px(size), &[run], None)
    };
    let value = shape(value, FONT);
    let raised = raised.map(|r| shape(r, FONT * 0.75));
    let value_width = f32::from(value.width);
    let width = value_width + raised.as_ref().map_or(0.0, |r| f32::from(r.width) + 0.5);
    let height = FONT * 1.3;
    let along = unit2(sub2(d.to[1], d.to[0]));
    // How far the label reaches from its center in a direction, with a little padding.
    let reach = |dir: [f32; 2]| (width / 2.0 + 2.0) * dir[0].abs() + height / 2.0 * dir[1].abs();
    let center = if len2(sub2(d.to[1], d.to[0])) >= width + 2.0 * TICK + 4.0 {
        add2(mul2(add2(d.to[0], d.to[1]), 0.5), mul2(d.out, 1.0 + reach(d.out)))
    } else {
        add2(d.to[1], mul2(along, RUN_ON + 2.0 + reach(along)))
    };
    let corner = sub2(center, [width / 2.0, height / 2.0]);
    window.paint_quad(fill(Bounds::new(at(sub2(corner, [2.0, 0.0])), size(px(width + 4.0), px(height))), background).corner_radii(px(2.0)));
    value.paint(at(corner), px(height), window, cx).ok();
    if let Some(raised) = raised {
        raised.paint(at([corner[0] + value_width + 0.5, corner[1] - FONT * 0.3]), px(height), window, cx).ok();
    }
}

/// A length as building drawings write it: meters with two decimals from
/// 1 m, centimeters below, a millimeter digit raised: "2,13" + "5", "36" + "5".
fn din(meters: f32) -> Option<(String, Option<String>)> {
    let mm = (meters * 1000.0).round() as i64;
    if mm <= 0 {
        return None;
    }
    let raised = (mm % 10 != 0).then(|| (mm % 10).to_string());
    let cm = mm / 10;
    let value = if cm >= 100 { format!("{},{:02}", cm / 100, cm % 100) } else { cm.to_string() };
    Some((value, raised))
}

/// The direction of an oblique tick: the line turned 45° counter-clockwise as
/// seen, "/" on a horizontal line and "\" on an upright one.
fn oblique(along: [f32; 2]) -> [f32; 2] {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    // Screen y points down.
    [(along[0] + along[1]) * s, (along[1] - along[0]) * s]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn mul(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn add2(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub2(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn mul2(a: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] * k, a[1] * k]
}

fn dot2(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross2(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn len2(a: [f32; 2]) -> f32 {
    dot2(a, a).sqrt()
}

fn unit2(a: [f32; 2]) -> [f32; 2] {
    let l = len2(a);
    if l > 0.0 { mul2(a, 1.0 / l) } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Camera;

    fn building() -> Frame {
        Frame { origin: [0.0; 3], axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], size: [10.0, 6.0, 3.0] }
    }

    fn view(camera: Camera) -> Projection {
        Projection::new([5.0, 3.0, 1.5], 6.0, camera, 800.0, 600.0)
    }

    fn values(dims: &[Dim]) -> Vec<f32> {
        let mut v: Vec<f32> = dims.iter().map(|d| d.value).collect();
        v.sort_by(f32::total_cmp);
        v
    }

    #[test]
    fn lengths_as_building_drawings_write_them() {
        let din = |m| din(m).map(|(v, r)| format!("{v}{}", r.map_or(String::new(), |r| format!("^{r}"))));
        assert_eq!(din(12.49).as_deref(), Some("12,49"));
        assert_eq!(din(2.135).as_deref(), Some("2,13^5"));
        assert_eq!(din(1.0).as_deref(), Some("1,00"));
        assert_eq!(din(0.365).as_deref(), Some("36^5"));
        assert_eq!(din(0.06).as_deref(), Some("6"));
        assert_eq!(din(0.0), None);
    }

    #[test]
    fn ticks_slant_like_a_slash() {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        // Up and to the right on a horizontal line, whichever way it runs.
        assert_eq!(oblique([1.0, 0.0]), [s, -s]);
        assert_eq!(oblique([-1.0, 0.0]), [-s, s]);
        // Up and to the left on an upright one.
        assert_eq!(oblique([0.0, -1.0]), [-s, -s]);
    }

    #[test]
    fn box_gets_length_width_and_height_outside_it() {
        let proj = view(Camera::default());
        let frame = building();
        let dims = layout(&frame, Style::Box, &proj);
        assert_eq!(values(&dims), [3.0, 6.0, 10.0]);
        let corners: Vec<[f32; 3]> =
            (0..8).map(|i| proj.point(frame.at([(i & 1) as f32 * 10.0, (i >> 1 & 1) as f32 * 6.0, (i >> 2) as f32 * 3.0]))).collect();
        let right = corners.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
        let middle = corners.iter().map(|p| p[1]).sum::<f32>() / 8.0;
        for d in &dims {
            let mid = mul2(add2(d.to[0], d.to[1]), 0.5);
            if d.value == 3.0 {
                assert!(mid[0] > right, "height stands right of the box: {d:?}");
            } else {
                assert!(mid[1] > middle, "plan sides run below the box: {d:?}");
            }
        }
    }

    #[test]
    fn side_seen_end_on_is_left_out_and_the_rest_stand_off_downward() {
        // Looking straight along y: the 6 m side has no length on screen, and
        // the ground is seen edge-on, so the 10 m side stands off below.
        let dims = layout(&building(), Style::Box, &view(Camera { yaw: 0.0, pitch: 0.0, zoom: 1.0, pan: [0.0, 0.0] }));
        assert_eq!(values(&dims), [3.0, 10.0]);
        let length = dims.iter().find(|d| d.value == 10.0).unwrap();
        assert!(length.out[1] > 0.99, "{length:?}");
    }

    #[test]
    fn wall_thickness_across_its_top() {
        let wall = Frame { origin: [0.0; 3], axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], size: [5.0, 0.365, 2.65] };
        let dims = layout(&wall, Style::Wall, &Projection::new([2.5, 0.2, 1.3], 3.0, Camera::default(), 800.0, 600.0));
        assert_eq!(values(&dims), [0.365, 2.65, 5.0]);
        let thickness = dims.iter().find(|d| d.value == 0.365).unwrap();
        assert!(thickness.out[1] < -0.99, "stands off upward: {thickness:?}");
    }
}
