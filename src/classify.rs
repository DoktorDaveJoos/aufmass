//! Turns 3D solids into walls, slabs and roofs with quantities.

use crate::acis::Body;
use crate::cad::Drawing;
use crate::config::Config;
use crate::geom::{area2, centroid, newell, point_in_polygon, ray_hit, round, Bbox, P2, V3};
use crate::model::{Element, Kind, Opening, OpeningKind, Skipped, Storey};
use std::collections::{BTreeMap, HashSet};

/// A face reduced to its plane. `normal` follows the outer loop's vertex order.
struct PFace<'a> {
    normal: V3,
    outer: &'a [V3],
    holes: Vec<&'a [V3]>,
    net_area: f64,
}

fn planar_faces(body: &Body) -> Vec<PFace<'_>> {
    body.faces
        .iter()
        .filter_map(|f| {
            let measured: Vec<(V3, f64)> = f.loops.iter().map(|l| newell(l)).collect();
            let outer_i = (0..f.loops.len()).max_by(|&a, &b| measured[a].1.total_cmp(&measured[b].1))?;
            let holes: Vec<&[V3]> = (0..f.loops.len()).filter(|&i| i != outer_i).map(|i| f.loops[i].as_slice()).collect();
            let holes_area: f64 = (0..f.loops.len()).filter(|&i| i != outer_i).map(|i| measured[i].1).sum();
            Some(PFace {
                normal: measured[outer_i].0,
                outer: &f.loops[outer_i],
                holes,
                net_area: measured[outer_i].1 - holes_area,
            })
        })
        .filter(|f| f.net_area > 1e-6)
        .collect()
}

fn extent(pts: &[V3], dir: V3) -> (f64, f64) {
    pts.iter().map(|p| p.dot(dir)).fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
}

fn mean(pts: &[V3]) -> V3 {
    pts.iter().fold(V3::default(), |a, p| a + *p) * (1.0 / pts.len() as f64)
}

/// Builds the elements of one floor plan file as the storey `name`.
pub fn storey(d: &Drawing, name: &str, cfg: &Config, seen: &mut HashSet<String>, skipped: &mut Vec<Skipped>) -> Storey {
    let mut skips: BTreeMap<(String, String), usize> = BTreeMap::new();
    let bodies: Vec<(&str, Kind, &Body, Bbox)> = d
        .solids
        .iter()
        .flat_map(|s| s.bodies.iter().map(move |b| (s.layer.as_str(), s.kind, b, Bbox::of(b.vertices()))))
        .collect();
    let existing = existing_mask(&bodies, &d.existing_marks);

    let mut elements = Vec::new();
    for (i, &(layer, kind, body, bb)) in bodies.iter().enumerate() {
        let mut skip = |reason: &str| *skips.entry((layer.to_string(), reason.to_string())).or_default() += 1;
        if existing[i] {
            skip("Bestand (graue Schraffur)");
            continue;
        }
        let key = format!("{layer}|{:.2}|{:.2}|{:.2}|{:.2}|{:.2}|{:.2}", bb.min.x, bb.min.y, bb.min.z, bb.max.x, bb.max.y, bb.max.z);
        if !seen.insert(key) {
            skip("Duplikat aus einer anderen Datei");
            continue;
        }
        let result = match kind {
            Kind::Wall => wall(body, cfg),
            Kind::Slab => slab(body, cfg, Kind::Slab),
            Kind::Roof => roof(body, cfg),
        };
        match result {
            Ok(mut e) => {
                e.layer = layer.to_string();
                e.faces = body.faces.iter().map(|f| f.loops.clone()).collect();
                elements.push(e);
            }
            Err(reason) => skip(reason),
        }
    }
    if d.unreadable_solids > 0 {
        skips.insert(("*".into(), "3D-Daten nicht lesbar".into()), d.unreadable_solids);
    }

    mark_exterior(&mut elements);
    name_and_number(name, &mut elements, cfg);
    skipped.extend(skips.into_iter().map(|((layer, reason), count)| Skipped { file: d.file.clone(), layer, reason, count }));
    Storey { name: name.to_string(), file: d.file.clone(), elements }
}

/// Marks bodies standing on an existing-building hatch, plus everything connected to them.
fn existing_mask(bodies: &[(&str, Kind, &Body, Bbox)], marks: &[Vec<P2>]) -> Vec<bool> {
    let mut parent: Vec<usize> = (0..bodies.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        if p[i] != i {
            let r = find(p, p[i]);
            p[i] = r;
        }
        p[i]
    }
    for i in 0..bodies.len() {
        for j in i + 1..bodies.len() {
            if bodies[i].3.touches(&bodies[j].3, 0.02) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let centers: Vec<P2> = marks.iter().map(|m| centroid(m)).collect();
    let mut seeds = HashSet::new();
    for (i, (_, kind, _, bb)) in bodies.iter().enumerate() {
        let on_mark = centers.iter().any(|c| {
            c.0 >= bb.min.x - 0.02 && c.0 <= bb.max.x + 0.02 && c.1 >= bb.min.y - 0.02 && c.1 <= bb.max.y + 0.02
        });
        if *kind == Kind::Wall && on_mark {
            seeds.insert(find(&mut parent, i));
        }
    }
    (0..bodies.len()).map(|i| seeds.contains(&find(&mut parent, i))).collect()
}

fn wall(body: &Body, cfg: &Config) -> Result<Element, &'static str> {
    let faces = planar_faces(body);
    let verts: Vec<V3> = body.vertices().copied().collect();
    let vertical: Vec<&PFace> = faces.iter().filter(|f| f.normal.z.abs() < 0.05).collect();
    // Thickness runs along the vertical-face normal with the smallest extent.
    let n0 = vertical
        .iter()
        .map(|f| V3::new(f.normal.x, f.normal.y, 0.0).unit())
        .min_by(|a, b| {
            let (ea, eb) = (extent(&verts, *a), extent(&verts, *b));
            (ea.1 - ea.0).total_cmp(&(eb.1 - eb.0))
        })
        .ok_or("Wand ohne senkrechte Flächen")?;
    let (lo, hi) = extent(&verts, n0);
    let thickness = hi - lo;
    if thickness < cfg.filters.min_wall_thickness {
        return Err("dünner als min_wall_thickness (Glas, Belag)");
    }
    let reference = vertical
        .iter()
        .filter(|f| f.normal.dot(n0).abs() > 0.99)
        .max_by(|a, b| a.net_area.total_cmp(&b.net_area))
        .ok_or("keine Wandfläche gefunden")?;
    let n = if mean(&verts).dot(n0) >= reference.outer[0].dot(n0) { n0 } else { -n0 };
    let w_face = reference.outer[0].dot(n);
    let u = V3::Z.cross(n).unit();
    let to2 = |p: &V3| (p.dot(u), p.z);
    let from2 = |s: f64, z: f64, w: f64| u * s + n * w + V3::Z * z;

    let outer: Vec<P2> = reference.outer.iter().map(to2).collect();
    let (s_min, s_max) = outer.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (z_min, z_max) = outer.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));

    let kind_of = |r: &[f64; 4]| {
        let f = &cfg.filters;
        if r[2] - z_min > f.door_max_sill {
            OpeningKind::Window
        } else if r[3] - r[2] >= f.min_door_height {
            OpeningKind::Door
        } else {
            OpeningKind::Other
        }
    };
    let mut rects: Vec<(OpeningKind, [f64; 4], f64)> = Vec::new();
    for hole in &reference.holes {
        let pts: Vec<P2> = hole.iter().map(to2).collect();
        let area = area2(&pts).abs();
        if area >= cfg.filters.min_opening_area {
            let r = bounds(&pts);
            rects.push((kind_of(&r), r, area));
        }
    }
    for r in cutouts(&outer, cfg.filters.door_max_sill) {
        let area = (r[1] - r[0]) * (r[3] - r[2]);
        if area >= cfg.filters.min_opening_area {
            rects.push((kind_of(&r), r, area));
        }
    }
    rects.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
    let openings: Vec<Opening> = rects
        .into_iter()
        .map(|(kind, [s0, s1, z0, z1], area)| Opening {
            id: String::new(),
            kind,
            width: round(s1 - s0, 3),
            height: round(z1 - z0, 3),
            sill: round(z0 - z_min, 3),
            area: round(area, 3),
            offset: round(s0 - s_min, 3),
            outline: vec![from2(s0, z0, w_face), from2(s1, z0, w_face), from2(s1, z1, w_face), from2(s0, z1, w_face)],
        })
        .collect();

    let openings_area: f64 = openings.iter().map(|o| o.area).sum();
    let w_center = w_face + thickness / 2.0;
    Ok(Element {
        id: String::new(),
        kind: Kind::Wall,
        name: String::new(),
        layer: String::new(),
        exterior: Some(false),
        thickness: round(thickness, 4),
        length: round(s_max - s_min, 3),
        height: round(z_max - z_min, 3),
        pitch_deg: None,
        bottom_z: round(z_min, 3),
        gross_area: round(reference.net_area + openings_area, 3),
        openings_area: round(openings_area, 3),
        net_area: round(reference.net_area, 3),
        volume: round(reference.net_area * thickness, 3),
        outline: reference.outer.to_vec(),
        holes: reference.holes.iter().map(|h| h.to_vec()).collect(),
        normal: n,
        axis: u,
        start: Some(from2(s_min, z_min, w_center)),
        end: Some(from2(s_max, z_min, w_center)),
        openings,
        faces: Vec::new(),
    })
}

/// Openings cut into a wall face from its bottom or its ends (doors, corner
/// windows), as rectangles [s0, s1, z0, z1] in face coordinates (s, z).
///
/// The uncut face is the upper convex envelope of the outline, closed along the
/// wall bottom. Each run of outline vertices off that envelope is one cut-out;
/// its rectangle spans the run plus its neighbours on the envelope. Cut-outs
/// touching only the top (steps under a roof) are not openings.
fn cutouts(outer: &[P2], max_sill: f64) -> Vec<[f64; 4]> {
    let [s_min, s_max, z_min, _] = bounds(outer);
    let mut gross = upper_hull(outer);
    gross.insert(0, (s_min, z_min));
    gross.push((s_max, z_min));
    let on_gross = |p: P2| {
        (0..gross.len()).any(|i| point_segment_distance(p, gross[i], gross[(i + 1) % gross.len()]) < 1e-3)
    };
    let flags: Vec<bool> = outer.iter().map(|&p| on_gross(p)).collect();
    let Some(start) = flags.iter().position(|&f| f) else { return vec![] };
    let n = outer.len();
    let mut rects = Vec::new();
    let mut run: Vec<P2> = Vec::new();
    for k in 1..=n {
        let i = (start + k) % n;
        if !flags[i] {
            if run.is_empty() {
                run.push(outer[(i + n - 1) % n]);
            }
            run.push(outer[i]);
        } else if !run.is_empty() {
            run.push(outer[i]);
            let r = bounds(&run);
            let at_bottom = r[2] - z_min <= max_sill;
            let at_end = (r[0] - s_min).abs() < 1e-3 || (r[1] - s_max).abs() < 1e-3;
            let below_top = r[3] < envelope_at(&gross, r[0]).min(envelope_at(&gross, r[1])) - 1e-3;
            if at_bottom || (at_end && below_top) {
                rects.push(r);
            }
            run.clear();
        }
    }
    rects
}

/// Upper convex hull from left to right.
fn upper_hull(points: &[P2]) -> Vec<P2> {
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut h: Vec<P2> = Vec::new();
    for p in pts {
        while h.len() >= 2 {
            let (o, a) = (h[h.len() - 2], h[h.len() - 1]);
            if (a.0 - o.0) * (p.1 - o.1) - (a.1 - o.1) * (p.0 - o.0) >= -1e-9 {
                h.pop();
            } else {
                break;
            }
        }
        h.push(p);
    }
    h
}

/// Highest point of a polyline at position `s`.
fn envelope_at(pts: &[P2], s: f64) -> f64 {
    let mut top = f64::MIN;
    for (i, a) in pts.iter().enumerate() {
        let b = pts[(i + 1) % pts.len()];
        let (lo, hi) = if a.0 <= b.0 { (*a, b) } else { (b, *a) };
        if hi.0 - lo.0 > 1e-9 && (lo.0..=hi.0).contains(&s) {
            top = top.max(lo.1 + (hi.1 - lo.1) * (s - lo.0) / (hi.0 - lo.0));
        } else if (a.0 - s).abs() < 1e-9 {
            top = top.max(a.1);
        }
    }
    top
}

fn point_segment_distance(p: P2, a: P2, b: P2) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

/// [x_min, x_max, y_min, y_max]
fn bounds(pts: &[P2]) -> [f64; 4] {
    pts.iter().fold([f64::MAX, f64::MIN, f64::MAX, f64::MIN], |b, p| [b[0].min(p.0), b[1].max(p.0), b[2].min(p.1), b[3].max(p.1)])
}

fn slab(body: &Body, cfg: &Config, kind: Kind) -> Result<Element, &'static str> {
    let faces = planar_faces(body);
    let verts: Vec<V3> = body.vertices().copied().collect();
    let bb = Bbox::of(&verts);
    let thickness = bb.max.z - bb.min.z;
    if thickness > cfg.filters.max_slab_thickness {
        return Err("dicker als max_slab_thickness (Möbel, Treppe, Einbau)");
    }
    let reference = faces
        .iter()
        .filter(|f| f.normal.z.abs() > 0.95)
        .max_by(|a, b| a.net_area.total_cmp(&b.net_area))
        .ok_or("keine waagerechte Fläche")?;
    if reference.net_area < cfg.filters.min_slab_area {
        return Err("kleiner als min_slab_area (Möbel, Einbau)");
    }
    let on_top = reference.outer[0].z > (bb.min.z + bb.max.z) / 2.0;
    let holes_area: f64 = reference.holes.iter().map(|h| newell(h).1).sum();
    Ok(Element {
        id: String::new(),
        kind,
        name: String::new(),
        layer: String::new(),
        exterior: None,
        thickness: round(thickness, 4),
        length: round((bb.max.x - bb.min.x).max(bb.max.y - bb.min.y), 3),
        height: round(thickness, 3),
        pitch_deg: None,
        bottom_z: round(bb.min.z, 3),
        gross_area: round(reference.net_area + holes_area, 3),
        openings_area: round(holes_area, 3),
        net_area: round(reference.net_area, 3),
        volume: round(reference.net_area * thickness, 3),
        outline: reference.outer.to_vec(),
        holes: reference.holes.iter().map(|h| h.to_vec()).collect(),
        normal: if on_top { -V3::Z } else { V3::Z },
        axis: longest_edge(reference.outer),
        start: None,
        end: None,
        openings: Vec::new(),
        faces: Vec::new(),
    })
}

fn roof(body: &Body, cfg: &Config) -> Result<Element, &'static str> {
    let faces = planar_faces(body);
    let verts: Vec<V3> = body.vertices().copied().collect();
    let Some(reference) = faces
        .iter()
        .filter(|f| (0.05..=0.95).contains(&f.normal.z.abs()))
        .max_by(|a, b| a.net_area.total_cmp(&b.net_area))
    else {
        return slab(body, cfg, Kind::Roof);
    };
    let (lo, hi) = extent(&verts, reference.normal);
    let n = if mean(&verts).dot(reference.normal) >= reference.outer[0].dot(reference.normal) {
        reference.normal
    } else {
        -reference.normal
    };
    let bb = Bbox::of(&verts);
    let axis = V3::Z.cross(n).unit();
    let (a0, a1) = extent(&verts, axis);
    let holes_area: f64 = reference.holes.iter().map(|h| newell(h).1).sum();
    let thickness = hi - lo;
    Ok(Element {
        id: String::new(),
        kind: Kind::Roof,
        name: String::new(),
        layer: String::new(),
        exterior: None,
        thickness: round(thickness, 4),
        length: round(a1 - a0, 3),
        height: round(bb.max.z - bb.min.z, 3),
        pitch_deg: Some(round(n.z.abs().acos().to_degrees(), 1)),
        bottom_z: round(bb.min.z, 3),
        gross_area: round(reference.net_area + holes_area, 3),
        openings_area: round(holes_area, 3),
        net_area: round(reference.net_area, 3),
        volume: round(reference.net_area * thickness, 3),
        outline: reference.outer.to_vec(),
        holes: reference.holes.iter().map(|h| h.to_vec()).collect(),
        normal: n,
        axis,
        start: None,
        end: None,
        openings: Vec::new(),
        faces: Vec::new(),
    })
}

fn longest_edge(pts: &[V3]) -> V3 {
    (0..pts.len())
        .map(|i| pts[(i + 1) % pts.len()] - pts[i])
        .max_by(|a, b| a.len().total_cmp(&b.len()))
        .map(V3::unit)
        .unwrap_or(V3::new(1.0, 0.0, 0.0))
}

/// Plan footprint of a wall: its bottom centerline widened to the thickness.
pub fn wall_footprint(e: &Element) -> Option<[P2; 4]> {
    let (s, t) = (e.start?, e.end?);
    let h = e.normal * (e.thickness / 2.0);
    Some([(s - h).xy(), (t - h).xy(), (t + h).xy(), (s + h).xy()])
}

/// A wall is exterior when rays leaving one of its faces hit no other wall.
/// Hits closer than 25 cm are layers of the same wall build-up (e.g. cladding).
fn mark_exterior(elements: &mut [Element]) {
    let prints: Vec<Option<[P2; 4]>> = elements.iter().map(wall_footprint).collect();
    for i in 0..elements.len() {
        let Some(own) = prints[i] else { continue };
        let e = &elements[i];
        let n = e.normal.xy();
        let half = e.thickness / 2.0 + 0.01;
        let mut exterior = false;
        for side in [1.0, -1.0] {
            let escapes = [0.25, 0.5, 0.75]
                .iter()
                .filter(|&&f| {
                    let c = ((own[0].0 + own[3].0) / 2.0, (own[0].1 + own[3].1) / 2.0);
                    let d = ((own[1].0 + own[2].0) / 2.0, (own[1].1 + own[2].1) / 2.0);
                    let origin = (c.0 + (d.0 - c.0) * f + n.0 * half * side, c.1 + (d.1 - c.1) * f + n.1 * half * side);
                    let dir = (n.0 * side, n.1 * side);
                    !prints.iter().enumerate().any(|(j, p)| {
                        j != i && p.is_some_and(|p| ray_hit(origin, dir, &p).is_some_and(|t| t > 0.25 && t < 40.0) && !point_in_polygon(origin, &p))
                    })
                })
                .count();
            exterior |= escapes >= 2;
        }
        elements[i].exterior = Some(exterior);
    }
}

fn name_and_number(storey: &str, elements: &mut [Element], cfg: &Config) {
    for e in elements.iter_mut() {
        e.name = cfg.name_for(e.kind, e.thickness, e.exterior);
    }
    let anchor = |e: &Element| e.start.unwrap_or_else(|| e.outline[0]);
    elements.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(b.exterior.cmp(&a.exterior))
            .then(b.thickness.total_cmp(&a.thickness))
            .then(a.bottom_z.total_cmp(&b.bottom_z))
            .then(anchor(b).y.total_cmp(&anchor(a).y))
            .then(anchor(a).x.total_cmp(&anchor(b).x))
    });
    let mut counters: BTreeMap<Kind, usize> = BTreeMap::new();
    for e in elements.iter_mut() {
        let n = counters.entry(e.kind).or_default();
        *n += 1;
        e.id = format!("{storey}-{}{:02}", e.kind.id_prefix(), n);
        for (k, o) in e.openings.iter_mut().enumerate() {
            o.id = format!("{}-O{}", e.id, k + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn door_reaching_the_floor() {
        // 4 m wall, 2.5 m high, door from 1.0 to 1.9, 2.1 high
        let outer = [(0.0, 0.0), (1.0, 0.0), (1.0, 2.1), (1.9, 2.1), (1.9, 0.0), (4.0, 0.0), (4.0, 2.5), (0.0, 2.5)];
        assert_eq!(cutouts(&outer, 0.15), vec![[1.0, 1.9, 0.0, 2.1]]);
    }

    #[test]
    fn corner_glazing_at_the_wall_end() {
        // glazing from s = 3.47 to the wall end, 2.35 high, lintel above
        let outer = [(0.0, 0.0), (3.47, 0.0), (3.47, 2.35), (9.5, 2.35), (9.5, 2.65), (0.0, 2.65)];
        assert_eq!(cutouts(&outer, 0.15), vec![[3.47, 9.5, 0.0, 2.35]]);
    }

    #[test]
    fn gable_wall_has_no_cutouts() {
        let outer = [(0.0, 0.0), (9.0, 0.0), (9.0, 1.5), (4.5, 5.4), (0.0, 1.5)];
        assert!(cutouts(&outer, 0.15).is_empty());
    }

    #[test]
    fn step_in_the_wall_top_is_not_an_opening() {
        let outer = [(0.0, 0.0), (4.0, 0.0), (4.0, 2.0), (2.0, 2.0), (2.0, 2.6), (0.0, 2.6)];
        assert!(cutouts(&outer, 0.15).is_empty());
    }
}
