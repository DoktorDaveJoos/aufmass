use crate::model::Kind;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const DEFAULT: &str = include_str!("../aufmass.toml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub default_units: String,
    #[serde(default)]
    pub existing_colors: Vec<u16>,
    pub layers: Layers,
    pub filters: Filters,
    #[serde(default)]
    pub naming: Vec<Naming>,
    #[serde(default)]
    pub papa: Papa,
    #[serde(default)]
    pub cadwork: Cadwork,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layers {
    pub wall: Vec<String>,
    pub slab: Vec<String>,
    pub roof: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filters {
    pub min_wall_thickness: f64,
    pub max_slab_thickness: f64,
    pub min_slab_area: f64,
    pub min_opening_area: f64,
    pub door_max_sill: f64,
    pub min_door_height: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Naming {
    pub kind: Kind,
    pub thickness: f64,
    pub name: String,
    /// Quantity of the GAEB position: net area (default) or volume.
    #[serde(default)]
    pub unit: Unit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    M2,
    M3,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Papa {
    /// The PAPA program the app starts for "In PAPA öffnen".
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cadwork {
    /// The cadwork 3d program the app starts for "In cadwork öffnen".
    pub path: Option<PathBuf>,
}

impl Config {
    /// Loads `path`, else `aufmass.toml` in the working directory or next to
    /// the executable, else the built-in defaults.
    pub fn load(path: Option<&Path>) -> Result<Config> {
        let found = match path {
            Some(p) => Some(p.to_path_buf()),
            None => {
                let beside_exe = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("aufmass.toml")));
                [Some(PathBuf::from("aufmass.toml")), beside_exe].into_iter().flatten().find(|p| p.is_file())
            }
        };
        let (text, origin) = match found {
            Some(p) => (std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?, p.display().to_string()),
            None => (DEFAULT.to_string(), "built-in defaults".to_string()),
        };
        toml::from_str(&text).with_context(|| format!("parsing config from {origin}"))
    }

    pub fn kind_of_layer(&self, layer: &str) -> Option<Kind> {
        let layer = layer.to_lowercase();
        let hit = |names: &[String]| names.iter().any(|n| n.to_lowercase() == layer);
        if hit(&self.layers.wall) {
            Some(Kind::Wall)
        } else if hit(&self.layers.slab) {
            Some(Kind::Slab)
        } else if hit(&self.layers.roof) {
            Some(Kind::Roof)
        } else {
            None
        }
    }

    pub fn units_to_meters(&self, insunits: i16) -> f64 {
        match insunits {
            4 => 0.001,
            5 => 0.01,
            6 => 1.0,
            1 => 0.0254,
            2 => 0.3048,
            _ => match self.default_units.as_str() {
                "mm" => 0.001,
                "cm" => 0.01,
                _ => 1.0,
            },
        }
    }

    /// GAEB quantity unit for an element name; only named build-ups can be priced by volume.
    pub fn unit_for(&self, kind: Kind, name: &str) -> Unit {
        self.naming.iter().find(|n| n.kind == kind && n.name == name).map_or(Unit::M2, |n| n.unit)
    }

    pub fn name_for(&self, kind: Kind, thickness: f64, exterior: Option<bool>) -> String {
        if let Some(n) = self.naming.iter().find(|n| n.kind == kind && (n.thickness - thickness).abs() < 0.004) {
            return n.name.clone();
        }
        let cm = format_cm(thickness);
        match (kind, exterior) {
            (Kind::Wall, Some(true)) => format!("Außenwand {cm} cm"),
            (Kind::Wall, _) => format!("Innenwand {cm} cm"),
            (Kind::Slab, _) => format!("Decke {cm} cm"),
            (Kind::Roof, _) => format!("Dach {cm} cm"),
        }
    }
}

/// 0.365 -> "36,5", 0.18 -> "18"
pub fn format_cm(m: f64) -> String {
    let cm = (m * 1000.0).round() / 10.0;
    if cm.fract().abs() < 1e-9 { format!("{cm:.0}") } else { format!("{cm:.1}").replace('.', ",") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let c = Config::load(None).unwrap();
        assert_eq!(c.kind_of_layer("WAND"), Some(Kind::Wall));
        assert_eq!(c.kind_of_layer("dachhaut"), Some(Kind::Roof));
        assert_eq!(c.name_for(Kind::Wall, 0.15, Some(false)), "Innenwand 15 cm");
        assert_eq!(c.name_for(Kind::Wall, 0.24, Some(true)), "Außenwand 24 cm");
    }
}
