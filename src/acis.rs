//! Turns the ACIS data of a 3DSOLID into planar face polygons in world coordinates.
//!
//! Only vertices are used, so curved edges become straight segments. That is
//! fine for walls, slabs and roof plates, which are planar boxes with holes.

use crate::geom::V3;
use acadrust::entities::acis::{
    SatCoedge, SatDocument, SatEdge, SatFace, SatLoop, SatLump, SatPointer, SatRecord,
    SatShell, SatToken, SatVertex, Sense,
};
use std::collections::HashSet;

/// A planar face: one or more closed loops (outer boundary and holes, unordered).
#[derive(Debug, Clone)]
pub struct Face {
    pub loops: Vec<Vec<V3>>,
}

/// One body (closed solid) of a 3DSOLID entity.
#[derive(Debug, Clone)]
pub struct Body {
    pub faces: Vec<Face>,
}

impl Body {
    pub fn vertices(&self) -> impl Iterator<Item = &V3> {
        self.faces.iter().flat_map(|f| f.loops.iter().flatten())
    }

    pub fn scaled(mut self, k: f64) -> Body {
        for p in self.faces.iter_mut().flat_map(|f| f.loops.iter_mut().flatten()) {
            *p = *p * k;
        }
        self
    }

    /// Enclosed volume (divergence theorem). The loops of a closed shell all run
    /// the same way round the solid, so their signed tetrahedra add up to the
    /// volume. Measured from the vertex mean to keep world coordinates out of the
    /// rounding.
    pub fn volume(&self) -> f64 {
        let n = self.vertices().count().max(1) as f64;
        let c = self.vertices().fold(V3::default(), |a, p| a + *p) * (1.0 / n);
        let mut v = 0.0;
        for l in self.faces.iter().flat_map(|f| &f.loops) {
            let p0 = l[0] - c;
            for w in l[1..].windows(2) {
                v += p0.dot((w[0] - c).cross(w[1] - c));
            }
        }
        (v / 6.0).abs()
    }
}

pub fn bodies(doc: &SatDocument) -> Vec<Body> {
    doc.bodies()
        .iter()
        .map(|body| {
            let tf = Transform::read(doc, body.transform());
            let mut faces = Vec::new();
            for lump in chain(doc, body.lump(), |r| SatLump::from_record(r).map(|l| l.next_lump())) {
                let Some(shell_ptr) = SatLump::from_record(lump).map(|l| l.shell()) else { continue };
                for shell in chain(doc, shell_ptr, |r| SatShell::from_record(r).map(|s| s.next_shell())) {
                    let Some(face_ptr) = SatShell::from_record(shell).map(|s| s.face()) else { continue };
                    for face in chain(doc, face_ptr, |r| SatFace::from_record(r).map(|f| f.next_face())) {
                        let loops = face_loops(doc, face, &tf);
                        if !loops.is_empty() {
                            faces.push(Face { loops });
                        }
                    }
                }
            }
            Body { faces }
        })
        .filter(|b| !b.faces.is_empty())
        .collect()
}

fn face_loops(doc: &SatDocument, face: &SatRecord, tf: &Transform) -> Vec<Vec<V3>> {
    let Some(first_loop) = SatFace::from_record(face).map(|f| f.first_loop()) else { return vec![] };
    chain(doc, first_loop, |r| SatLoop::from_record(r).map(|l| l.next_loop()))
        .into_iter()
        .filter_map(|lp| {
            let start = SatLoop::from_record(lp)?.first_coedge();
            let pts: Vec<V3> = chain(doc, start, |r| SatCoedge::from_record(r).map(|c| c.next()))
                .into_iter()
                .filter_map(|ce| coedge_start(doc, ce))
                .map(|p| tf.apply(p))
                .collect();
            (pts.len() >= 3).then_some(pts)
        })
        .collect()
}

/// The vertex a coedge starts at, respecting its sense relative to the edge.
fn coedge_start(doc: &SatDocument, coedge: &SatRecord) -> Option<V3> {
    let ce = SatCoedge::from_record(coedge)?;
    let edge = SatEdge::from_record(doc.resolve(ce.edge())?)?;
    let vertex_ptr = match ce.sense() {
        Sense::Forward => edge.start_vertex(),
        Sense::Reversed => edge.end_vertex(),
    };
    let vertex = SatVertex::from_record(doc.resolve(vertex_ptr)?)?;
    let point = doc.resolve(vertex.point())?;
    point_position(point)
}

/// Point records are a single position token in SAB and three floats in SAT text.
fn point_position(point: &SatRecord) -> Option<V3> {
    if let Some(([x, y, z], 3)) = point.tokens.iter().find_map(|t| t.coordinate_components()) {
        return Some(V3::new(x, y, z));
    }
    let f: Vec<f64> = point.tokens.iter().filter_map(|t| t.as_float()).collect();
    (f.len() >= 3).then(|| V3::new(f[0], f[1], f[2]))
}

/// Follows a linked list of records until it ends or cycles.
fn chain<'a>(
    doc: &'a SatDocument,
    start: SatPointer,
    next: impl Fn(&'a SatRecord) -> Option<SatPointer>,
) -> Vec<&'a SatRecord> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut ptr = start;
    while let Some(rec) = doc.resolve(ptr) {
        if !seen.insert(rec.index) || out.len() > 100_000 {
            break;
        }
        out.push(rec);
        match next(rec) {
            Some(p) => ptr = p,
            None => break,
        }
    }
    out
}

/// ACIS body transform: `p' = (p · R) * scale + t`.
struct Transform {
    r: [f64; 9],
    t: [f64; 3],
    s: f64,
}

impl Transform {
    const IDENTITY: Transform = Transform { r: [1., 0., 0., 0., 1., 0., 0., 0., 1.], t: [0.; 3], s: 1.0 };

    /// Standard SAT stores 13 numbers as separate tokens. ODA's exporter stores them
    /// as one string ("1 0 0 0 1 0 0 0 1 tx ty tz scale no_rotate ..."): a plain
    /// string in SAT text (DXF up to 2010), length-prefixed bytes in SAB (DWG, DXF 2013+).
    fn read(doc: &SatDocument, ptr: SatPointer) -> Transform {
        let Some(rec) = doc.resolve(ptr).filter(|r| r.is_a("transform")) else { return Self::IDENTITY };
        let numbers = |text: &str| text.split_whitespace().filter_map(|w| w.parse::<f64>().ok()).collect::<Vec<_>>();
        let mut nums = Vec::new();
        for tok in &rec.tokens {
            match tok {
                SatToken::Sab { tag: 18, data } => {
                    let text = match data.get(..4) {
                        Some(len) if u32::from_le_bytes(len.try_into().unwrap()) as usize == data.len() - 4 => &data[4..],
                        _ => &data[..],
                    };
                    nums.extend(numbers(&String::from_utf8_lossy(text)));
                }
                SatToken::String(text) => nums.extend(numbers(text)),
                _ => nums.extend(tok.as_float()),
            }
        }
        if nums.len() < 13 {
            return Self::IDENTITY;
        }
        Transform { r: nums[0..9].try_into().unwrap(), t: [nums[9], nums[10], nums[11]], s: nums[12] }
    }

    fn apply(&self, p: V3) -> V3 {
        let r = &self.r;
        V3::new(
            (p.x * r[0] + p.y * r[3] + p.z * r[6]) * self.s + self.t[0],
            (p.x * r[1] + p.y * r[4] + p.z * r[7]) * self.s + self.t[1],
            (p.x * r[2] + p.y * r[5] + p.z * r[8]) * self.s + self.t[2],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loops (outer and holes) extruded by `d`, as a closed body.
    fn prism(base: &[Vec<V3>], d: V3) -> Body {
        let mut faces = vec![
            Face { loops: base.iter().map(|l| l.iter().rev().copied().collect()).collect() },
            Face { loops: base.iter().map(|l| l.iter().map(|p| *p + d).collect()).collect() },
        ];
        for l in base {
            for (i, &a) in l.iter().enumerate() {
                let b = l[(i + 1) % l.len()];
                faces.push(Face { loops: vec![vec![a, b, b + d, a + d]] });
            }
        }
        Body { faces }
    }

    #[test]
    fn volume_of_a_slab_with_a_hole() {
        let o = V3::new(1000.0, 2000.0, 3.0);
        let outer = [(0.0, 0.0), (4.0, 0.0), (4.0, 3.0), (0.0, 3.0)].map(|(x, y)| o + V3::new(x, y, 0.0)).to_vec();
        let hole = [(1.0, 1.0), (1.0, 2.0), (2.0, 2.0), (2.0, 1.0)].map(|(x, y)| o + V3::new(x, y, 0.0)).to_vec();
        let v = prism(&[outer, hole], V3::new(0.0, 0.0, 0.2)).volume();
        assert!((v - 2.2).abs() < 1e-9, "{v}");
    }

    #[test]
    fn volume_of_a_knee_wall_cut_by_the_roof() {
        // 36.5 cm thick, 1.569 m high outside, 1.252 m inside, 10 m long
        let profile = [(0.0, 0.0), (0.365, 0.0), (0.365, 1.252), (0.0, 1.569)].map(|(y, z)| V3::new(0.0, y, z)).to_vec();
        let v = prism(&[profile], V3::new(10.0, 0.0, 0.0)).volume();
        assert!((v - 10.0 * 0.365 * (1.252 + 1.569) / 2.0).abs() < 1e-9, "{v}");
    }
}
