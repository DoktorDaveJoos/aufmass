//! Plan preview: the original linework with the detected elements on top,
//! so a person can check the detection against the drawing.

use crate::classify::wall_footprint;
use crate::gaeb::esc;
use crate::geom::P2;
use crate::report::de;
use crate::model::{Building, Kind, OpeningKind, Storey};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

const PALETTE: [&str; 10] = [
    "#c0601e", "#1f8a70", "#7b4fa0", "#34495e", "#b03a2e", "#2e8b57", "#d4a017", "#2471a3", "#7f8c8d", "#c2185b",
];
const PX_PER_M: f64 = 60.0;

/// One color per wall name, in order of first appearance across all storeys.
pub fn wall_colors(b: &Building) -> BTreeMap<String, &'static str> {
    let mut colors = BTreeMap::new();
    for e in b.storeys.iter().flat_map(|s| &s.elements).filter(|e| e.kind == Kind::Wall) {
        let next = PALETTE[colors.len() % PALETTE.len()];
        colors.entry(e.name.clone()).or_insert(next);
    }
    colors
}

pub fn render(storey: &Storey, linework: &[[P2; 2]], colors: &BTreeMap<String, &'static str>) -> String {
    let mut s = String::new();
    draw(&mut s, storey, linework, colors).expect("writing to a String cannot fail");
    s
}

fn draw(s: &mut String, storey: &Storey, linework: &[[P2; 2]], colors: &BTreeMap<String, &'static str>) -> std::fmt::Result {
    let pts: Vec<P2> = storey.elements.iter().flat_map(|e| e.outline.iter().map(|p| p.xy())).collect();
    if pts.is_empty() {
        return Ok(());
    }
    let margin = 1.5;
    let (x0, x1, y0, y1) = pts.iter().fold((f64::MAX, f64::MIN, f64::MAX, f64::MIN), |b, p| {
        (b.0.min(p.0), b.1.max(p.0), b.2.min(p.1), b.3.max(p.1))
    });
    let (x0, x1, y0, y1) = (x0 - margin, x1 + margin, y0 - margin, y1 + margin);

    let names: BTreeSet<&str> = storey.elements.iter().filter(|e| e.kind == Kind::Wall).map(|e| e.name.as_str()).collect();
    let color_of = |name: &str| colors.get(name).copied().unwrap_or("#888888");
    let legend_h = 60.0 + 18.0 * names.len() as f64;
    let (w, h) = ((x1 - x0) * PX_PER_M, (y1 - y0) * PX_PER_M + legend_h);
    let map = |p: P2| ((p.0 - x0) * PX_PER_M, (y1 - p.1) * PX_PER_M + legend_h);
    let poly = |ps: &[P2]| ps.iter().map(|&p| { let (x, y) = map(p); format!("{x:.1},{y:.1}") }).collect::<Vec<_>>().join(" ");

    writeln!(s, r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}" font-family="Helvetica, Arial, sans-serif">"##)?;
    writeln!(s, r##"<rect width="100%" height="100%" fill="#ffffff"/>"##)?;

    writeln!(s, r##"<g stroke="#b8b8b8" stroke-width="0.6">"##)?;
    for [a, b] in linework {
        let inside = |p: &P2| p.0 >= x0 && p.0 <= x1 && p.1 >= y0 && p.1 <= y1;
        if inside(a) || inside(b) {
            let ((ax, ay), (bx, by)) = (map(*a), map(*b));
            writeln!(s, r##"<line x1="{ax:.1}" y1="{ay:.1}" x2="{bx:.1}" y2="{by:.1}"/>"##)?;
        }
    }
    writeln!(s, "</g>")?;

    for e in storey.elements.iter().filter(|e| e.kind != Kind::Wall) {
        let (fill, stroke) = if e.kind == Kind::Slab { ("#5b9bd5", "#2e6da4") } else { ("#e07b6b", "#a8432f") };
        let outline: Vec<P2> = e.outline.iter().map(|p| p.xy()).collect();
        writeln!(
            s,
            r##"<polygon points="{}" fill="{fill}" fill-opacity="0.08" stroke="{stroke}" stroke-width="1" stroke-dasharray="6 4"><title>{} {} – {} m²</title></polygon>"##,
            poly(&outline), esc(&e.id), esc(&e.name), de(e.net_area, 2)
        )?;
    }

    for e in storey.elements.iter().filter(|e| e.kind == Kind::Wall) {
        let Some(fp) = wall_footprint(e) else { continue };
        let color = color_of(&e.name);
        writeln!(
            s,
            r##"<polygon points="{}" fill="{color}" fill-opacity="0.85" stroke="#222" stroke-width="0.5"><title>{} {} – L {} m, H {} m, {} m² netto</title></polygon>"##,
            poly(&fp), esc(&e.id), esc(&e.name), de(e.length, 2), de(e.height, 2), de(e.net_area, 2)
        )?;
        let (start, half) = (e.start.unwrap(), e.normal * (e.thickness / 2.0 + 0.02));
        for o in &e.openings {
            let a = start + e.axis * o.offset;
            let b = start + e.axis * (o.offset + o.width);
            let rect = [(a - half).xy(), (b - half).xy(), (b + half).xy(), (a + half).xy()];
            let stroke = match o.kind {
                OpeningKind::Door => "#c0392b",
                OpeningKind::Window => "#2471a3",
                OpeningKind::Other => "#e08e0b",
            };
            writeln!(
                s,
                r##"<polygon points="{}" fill="#ffffff" stroke="{stroke}" stroke-width="1.2"><title>{} {} {} × {} m, Brüstung {} m</title></polygon>"##,
                poly(&rect), esc(&o.id), o.kind.label(), de(o.width, 2), de(o.height, 2), de(o.sill, 2)
            )?;
        }
    }

    writeln!(s, r##"<g font-size="9" fill="#111" text-anchor="middle">"##)?;
    for e in storey.elements.iter().filter(|e| e.kind == Kind::Wall && e.thickness > 0.1) {
        let (a, b) = (e.start.unwrap(), e.end.unwrap());
        let (x, y) = map(((a.x + b.x) / 2.0, (a.y + b.y) / 2.0));
        let short = e.id.rsplit('-').next().unwrap_or(&e.id);
        writeln!(s, r##"<text x="{x:.1}" y="{:.1}" stroke="#fff" stroke-width="2.5" paint-order="stroke">{}</text>"##, y + 3.0, esc(short))?;
    }
    writeln!(s, "</g>")?;

    writeln!(
        s,
        r##"<text x="12" y="22" font-size="15" font-weight="bold" fill="#111">{} ({}) – erkannte Bauteile</text>"##,
        esc(&storey.name), esc(&storey.file)
    )?;
    let mut y = 42.0;
    for name in &names {
        let area: f64 = storey.elements.iter().filter(|e| e.name == *name).map(|e| e.net_area).sum();
        writeln!(s, r##"<rect x="12" y="{:.0}" width="14" height="10" fill="{}"/>"##, y - 9.0, color_of(name))?;
        writeln!(s, r##"<text x="32" y="{y:.0}" font-size="11" fill="#111">{}: {} m² netto</text>"##, esc(name), de(area, 2))?;
        y += 18.0;
    }
    writeln!(
        s,
        r##"<text x="12" y="{y:.0}" font-size="11" fill="#555">Öffnungen weiß (rot = Tür, blau = Fenster, orange = sonstige) · Decken blau, Dach rot gestrichelt · grau = Originalplan</text>"##
    )?;
    writeln!(s, "</svg>")
}
