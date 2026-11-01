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
