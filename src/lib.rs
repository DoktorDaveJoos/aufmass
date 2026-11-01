//! Converts architect DWG/DXF plans that contain 3D solids into a building
//! model with quantities. Used by the `aufmass` CLI and the desktop app.

pub mod acis;
pub mod cad;
pub mod cadwork;
pub mod classify;
pub mod config;
pub mod gaeb;
pub mod geom;
pub mod model;
pub mod report;
pub mod svg;

pub use config::Config;

use anyhow::{bail, Context, Result};
use geom::P2;
use model::{Building, Storey};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Result of reading a set of plan files.
pub struct Conversion {
    pub building: Building,
    pub files: Vec<FileReport>,
    /// 2D plan lines per storey name, for the previews.
    linework: Vec<(String, Vec<[P2; 2]>)>,
    colors: BTreeMap<String, &'static str>,
    units: BTreeMap<(model::Kind, String), config::Unit>,
}

/// What one input file contributed.
pub struct FileReport {
    pub file: String,
    pub solids: usize,
    pub elements: usize,
    pub unreadable: usize,
    pub error: Option<String>,
}

impl FileReport {
    pub fn line(&self) -> String {
        if let Some(e) = &self.error {
            return format!("{}: {e}", self.file);
        }
        let unreadable = match self.unreadable {
            0 => String::new(),
            n => format!(" ({n} 3D-Körper ohne lesbare Daten)"),
        };
        format!("{:<14} {:>4} 3D-Körper auf Bauteil-Layern -> {:>3} Bauteile{unreadable}", self.file, self.solids, self.elements)
    }
}

/// All .dwg/.dxf files in the given files and folders (recursively), sorted.
pub fn collect_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    fn walk(p: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        // macOS metadata ("._eg.dwg" on USB sticks and in zips) and Office lock files ("~$...").
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with("._") || name.starts_with("~$") {
            return Ok(());
        }
        if p.is_dir() {
            for entry in std::fs::read_dir(p).with_context(|| format!("Ordner {} konnte nicht gelesen werden", p.display()))? {
                walk(&entry?.path(), out)?;
            }
        } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("dwg") || e.eq_ignore_ascii_case("dxf")) {
            out.push(p.to_path_buf());
        }
        Ok(())
    }
    let mut files = Vec::new();
    for p in inputs {
        if !p.exists() {
            bail!("{} existiert nicht", p.display());
        }
        walk(p, &mut files)?;
    }
    files.sort();
    if files.is_empty() {
        bail!("Keine .dwg- oder .dxf-Dateien gefunden.");
    }
    Ok(files)
}

pub const GAEB_FILE: &str = "mengen.x83";

/// Project name for the GAEB file: the plan folder the output sits in.
fn project_name(out: &Path) -> String {
    let abs = std::path::absolute(out).unwrap_or_else(|_| out.to_path_buf());
    abs.parent().and_then(|p| p.file_name()).map_or_else(|| "Aufmaß".to_string(), |n| n.to_string_lossy().into_owned())
}

/// The `aufmass` folder next to the first input.
pub fn default_out(first: &Path) -> PathBuf {
    let base = if first.is_dir() { first } else { first.parent().unwrap_or(Path::new(".")) };
    base.join("aufmass")
}

/// Whether a file name names a storey: "EG", "02_OG", "1.OG", "Grundriss Dachgeschoss".
fn names_storey(path: &Path) -> bool {
    const STOREYS: [&str; 15] = [
        "kg", "ug", "eg", "og", "dg", "sg", "sb", "keller", "kellergeschoss", "untergeschoss", "erdgeschoss",
        "obergeschoss", "dachgeschoss", "staffelgeschoss", "spitzboden",
    ];
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
    stem.split(|c: char| !c.is_alphanumeric()).any(|w| STOREYS.contains(&w.trim_matches(|c: char| c.is_ascii_digit())))
}

/// Reads all files. Unreadable files are reported, not fatal; call
/// [`Conversion::check`] to find out whether anything usable came out.
pub fn convert(files: &[PathBuf], cfg: &Config) -> Conversion {
    let mut building = Building { units: "m", storeys: Vec::new(), skipped: Vec::new() };
    let mut seen = HashSet::new();
    let mut linework = Vec::new();
    let mut reports: Vec<Option<FileReport>> = files.iter().map(|_| None).collect();
    // A solid found in several files (the site plan repeats the ground floor)
    // belongs to the first file that has it, so floor plans are read first.
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by_key(|&i| !names_storey(&files[i]));
    for i in order {
        let f = &files[i];
        let drawing = match cad::load(f, cfg) {
            Ok(d) => d,
            Err(e) => {
                reports[i] = Some(FileReport {
                    file: f.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                    solids: 0,
                    elements: 0,
                    unreadable: 0,
                    error: Some(format!("{e:#}")),
                });
                continue;
            }
        };
        // Same-named files in different folders get distinct storeys, previews and GAEB titles.
        let base = drawing.stem.to_uppercase();
        let mut name = base.clone();
        for n in 2.. {
            if !building.storeys.iter().any(|s| s.name == name) {
                break;
            }
            name = format!("{base}-{n}");
        }
        let storey = classify::storey(&drawing, &name, cfg, &mut seen, &mut building.skipped);
        reports[i] = Some(FileReport {
            file: drawing.file.clone(),
            solids: drawing.solids.iter().map(|s| s.bodies.len()).sum(),
            elements: storey.elements.len(),
            unreadable: drawing.unreadable_solids,
            error: None,
        });
        if !storey.elements.is_empty() {
            linework.push((storey.name.clone(), drawing.linework));
            building.storeys.push(storey);
        }
    }
    let lowest = |s: &Storey| s.elements.iter().map(|e| e.bottom_z).fold(f64::MAX, f64::min);
    building.storeys.sort_by(|a, b| lowest(a).total_cmp(&lowest(b)));
    let colors = svg::wall_colors(&building);
    let units = building
        .storeys
        .iter()
        .flat_map(|s| &s.elements)
        .map(|e| ((e.kind, e.name.clone()), cfg.unit_for(e.kind, &e.name)))
        .collect();
    Conversion { building, files: reports.into_iter().flatten().collect(), linework, colors, units }
}

impl Conversion {
    /// Fails with an explanation for the user when no building elements were found.
    pub fn check(&self) -> Result<()> {
        if !self.building.storeys.is_empty() {
            return Ok(());
        }
        let unreadable: usize = self.files.iter().map(|r| r.unreadable).sum();
        if unreadable > 0 {
            bail!(
                "Die Dateien enthalten {unreadable} 3D-Körper, deren Daten nicht lesbar sind. \
                 Das passiert, wenn eine Datei durch einen Konverter gelaufen ist. \
                 Bitte die Originaldatei aus dem CAD-Programm des Architekten verwenden (DWG oder DXF)."
            );
        }
        bail!(
            "Keine 3D-Wände, -Decken oder -Dächer gefunden. Reine 2D-Zeichnungen werden noch nicht \
             unterstützt. Bitte beim Architekten DWG/DXF mit 3D-Modell anfragen, Layer siehe aufmass.toml."
        )
    }

    /// Wall colors by element name, shared by the plan previews and the 3D view.
    pub fn colors(&self) -> &BTreeMap<String, &'static str> {
        &self.colors
    }

    pub fn preview_svg(&self, storey: &Storey) -> String {
        let lines = self.linework.iter().find(|(n, _)| *n == storey.name).map(|(_, l)| l.as_slice()).unwrap_or(&[]);
        svg::render(storey, lines, &self.colors)
    }

    /// Writes building.json, mengen.xlsx, mengen.x83 and one plan preview per storey.
    pub fn write(&self, out: &Path) -> Result<()> {
        std::fs::create_dir_all(out).with_context(|| format!("Ordner {} konnte nicht angelegt werden", out.display()))?;
        let saving = |name: &str| format!("{name} konnte nicht gespeichert werden. Ist die Datei noch in einem anderen Programm geöffnet?");
        std::fs::write(out.join("building.json"), serde_json::to_string_pretty(&self.building)?).with_context(|| saving("building.json"))?;
        report::write_xlsx(&self.building, &out.join("mengen.xlsx")).with_context(|| saving("mengen.xlsx"))?;
        gaeb::write_x83(&self.building, &self.units, &project_name(out), &gaeb::today(), &out.join(GAEB_FILE))
            .with_context(|| saving(GAEB_FILE))?;
        // Only a convenience for the cadwork plugin; the outputs are complete without it.
        cadwork::remember_output(&out.join("building.json")).ok();
        for s in &self.building.storeys {
            let file = format!("vorschau-{}.svg", s.name);
            std::fs::write(out.join(&file), self.preview_svg(s)).with_context(|| saving(&file))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storey_file_names() {
        for n in ["eg.dwg", "02_EG.dwg", "1.OG.dxf", "OG2.dwg", "Grundriss Dachgeschoss.dwg", "KG-neu.dwg"] {
            assert!(names_storey(Path::new(n)), "{n}");
        }
        for n in ["lage.dwg", "01_Lageplan.dwg", "schnitt.dwg", "nw.dwg", "Ansicht Süd.dwg"] {
            assert!(!names_storey(Path::new(n)), "{n}");
        }
    }
}
