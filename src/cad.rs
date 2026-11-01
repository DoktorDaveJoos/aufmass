//! Reads a DWG/DXF file and keeps what the converter needs.

use crate::acis::{self, Body};
use crate::config::Config;
use crate::geom::P2;
use crate::model::Kind;
use acadrust::entities::{BoundaryEdge, EntityType};
use acadrust::{DwgReader, DxfReader};
use anyhow::{anyhow, bail, Result};
use std::path::Path;

pub struct Drawing {
    pub file: String,
    pub stem: String,
    pub solids: Vec<SolidIn>,
    /// Hatch outlines whose color marks an existing building.
    pub existing_marks: Vec<Vec<P2>>,
    /// 2D lines of the plan, for the preview background.
    pub linework: Vec<[P2; 2]>,
    pub unreadable_solids: usize,
}

pub struct SolidIn {
    pub layer: String,
    pub kind: Kind,
    pub bodies: Vec<Body>,
}

pub fn load(path: &Path, cfg: &Config) -> Result<Drawing> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let doc = match ext.as_str() {
        "dwg" => DwgReader::from_file(path).and_then(|mut r| r.read()),
        "dxf" => DxfReader::from_file(path).and_then(|r| r.read()),
        _ => bail!("{}: nur DWG und DXF werden gelesen", path.display()),
    }
    .map_err(|e| anyhow!("{} konnte nicht gelesen werden: {e}", path.display()))?;
    let k = cfg.units_to_meters(doc.header.insertion_units);

    let mut d = Drawing {
        file: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        stem: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
        solids: Vec::new(),
        existing_marks: Vec::new(),
        linework: Vec::new(),
        unreadable_solids: 0,
    };
    for e in doc.model_space_entities() {
        match e {
            EntityType::Solid3D(s) => {
                let Some(kind) = cfg.kind_of_layer(&s.common.layer) else { continue };
                // Converters such as LibreDWG keep the entity but drop its ACIS data.
                match s.acis_data.parse().map(|sat| acis::bodies(&sat)) {
                    Some(bodies) if !bodies.is_empty() => d.solids.push(SolidIn {
                        layer: s.common.layer.clone(),
                        kind,
                        bodies: bodies.into_iter().map(|b| b.scaled(k)).collect(),
                    }),
                    _ => d.unreadable_solids += 1,
                }
            }
            EntityType::Hatch(h) if h.common.color.index().is_some_and(|c| cfg.existing_colors.contains(&c)) => {
                for path in &h.paths {
                    let mut pts = Vec::new();
                    for edge in &path.edges {
                        match edge {
                            BoundaryEdge::Polyline(pl) => pts.extend(pl.vertices.iter().map(|v| (v.x * k, v.y * k))),
                            BoundaryEdge::Line(l) => pts.push((l.start.x * k, l.start.y * k)),
                            _ => {}
                        }
                    }
                    if pts.len() >= 3 {
                        d.existing_marks.push(pts);
                    }
                }
            }
            EntityType::Line(l) => d.linework.push([(l.start.x * k, l.start.y * k), (l.end.x * k, l.end.y * k)]),
            EntityType::LwPolyline(p) => {
                let pts: Vec<P2> = p.vertices.iter().map(|v| (v.location.x * k, v.location.y * k)).collect();
                d.linework.extend(pts.windows(2).map(|w| [w[0], w[1]]));
                if p.is_closed && pts.len() > 2 {
                    d.linework.push([pts[pts.len() - 1], pts[0]]);
                }
            }
            _ => {}
        }
    }
    Ok(d)
}
