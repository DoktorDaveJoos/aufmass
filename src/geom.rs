//! Small vector and polygon helpers. All lengths are meters.

use serde::{Serialize, Serializer};
use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl V3 {
    pub const Z: V3 = V3::new(0.0, 0.0, 1.0);

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, o: V3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn unit(self) -> V3 {
        let l = self.len();
        if l > 0.0 { self * (1.0 / l) } else { self }
    }

    pub fn xy(self) -> P2 {
        (self.x, self.y)
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f64> for V3 {
    type Output = V3;
    fn mul(self, k: f64) -> V3 {
        V3::new(self.x * k, self.y * k, self.z * k)
    }
}

impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        self * -1.0
    }
}

/// Serialized as `[x, y, z]`, rounded to 0.1 mm.
impl Serialize for V3 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        [round(self.x, 4), round(self.y, 4), round(self.z, 4)].serialize(s)
    }
}

pub fn round(v: f64, decimals: i32) -> f64 {
    let k = 10f64.powi(decimals);
    (v * k).round() / k
}

/// Unit normal and area of a planar 3D polygon (Newell's method).
/// The normal follows the right-hand rule of the vertex order.
pub fn newell(pts: &[V3]) -> (V3, f64) {
    let mut n = V3::default();
    for (i, a) in pts.iter().enumerate() {
        let b = pts[(i + 1) % pts.len()];
        n.x += (a.y - b.y) * (a.z + b.z);
        n.y += (a.z - b.z) * (a.x + b.x);
        n.z += (a.x - b.x) * (a.y + b.y);
    }
    let l = n.len();
    (n.unit(), l / 2.0)
}

pub type P2 = (f64, f64);

/// Signed shoelace area (positive for counter-clockwise).
pub fn area2(p: &[P2]) -> f64 {
    let mut a = 0.0;
    for (i, &(x0, y0)) in p.iter().enumerate() {
        let (x1, y1) = p[(i + 1) % p.len()];
        a += x0 * y1 - x1 * y0;
    }
    a / 2.0
}

pub fn point_in_polygon(p: P2, poly: &[P2]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > p.1) != (yj > p.1) && p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn centroid(p: &[P2]) -> P2 {
    let n = p.len() as f64;
    (p.iter().map(|q| q.0).sum::<f64>() / n, p.iter().map(|q| q.1).sum::<f64>() / n)
}

/// Axis-aligned 3D bounding box.
#[derive(Clone, Copy, Debug)]
pub struct Bbox {
    pub min: V3,
    pub max: V3,
}

impl Bbox {
    pub fn of<'a>(pts: impl IntoIterator<Item = &'a V3>) -> Bbox {
        let mut b = Bbox { min: V3::new(f64::MAX, f64::MAX, f64::MAX), max: V3::new(f64::MIN, f64::MIN, f64::MIN) };
        for p in pts {
            b.min = V3::new(b.min.x.min(p.x), b.min.y.min(p.y), b.min.z.min(p.z));
            b.max = V3::new(b.max.x.max(p.x), b.max.y.max(p.y), b.max.z.max(p.z));
        }
        b
    }

    pub fn touches(&self, o: &Bbox, tol: f64) -> bool {
        self.min.x <= o.max.x + tol && o.min.x <= self.max.x + tol
            && self.min.y <= o.max.y + tol && o.min.y <= self.max.y + tol
            && self.min.z <= o.max.z + tol && o.min.z <= self.max.z + tol
    }
}

/// Distance along a 2D ray to a convex polygon, if it is hit.
pub fn ray_hit(origin: P2, dir: P2, poly: &[P2]) -> Option<f64> {
    let mut best: Option<f64> = None;
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let e = (b.0 - a.0, b.1 - a.1);
        let denom = dir.0 * e.1 - dir.1 * e.0;
        if denom.abs() < 1e-12 {
            continue;
        }
        let w = (a.0 - origin.0, a.1 - origin.1);
        let t = (w.0 * e.1 - w.1 * e.0) / denom;
        let u = (w.0 * dir.1 - w.1 * dir.0) / denom;
        if t > 0.0 && (0.0..=1.0).contains(&u) {
            best = Some(best.map_or(t, |b: f64| b.min(t)));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newell_area_of_vertical_rectangle() {
        let pts = [V3::new(0., 0., 0.), V3::new(3., 0., 0.), V3::new(3., 0., 2.), V3::new(0., 0., 2.)];
        let (n, a) = newell(&pts);
        assert!((a - 6.0).abs() < 1e-9);
        assert!((n.y.abs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ray_hits_square() {
        let sq = [(2.0, -1.0), (3.0, -1.0), (3.0, 1.0), (2.0, 1.0)];
        assert_eq!(ray_hit((0.0, 0.0), (1.0, 0.0), &sq), Some(2.0));
        assert_eq!(ray_hit((0.0, 0.0), (-1.0, 0.0), &sq), None);
    }
}
