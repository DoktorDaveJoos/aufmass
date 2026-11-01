//! The neutral building model written to `building.json`.
//! Coordinates are world coordinates in meters, z up.

use crate::geom::V3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Wall,
    Slab,
    Roof,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Wall, Kind::Slab, Kind::Roof];

    /// Heading for a group of elements, e.g. "Summe Wände".
    pub fn plural(self) -> &'static str {
        match self {
            Kind::Wall => "Wände",
            Kind::Slab => "Decken",
            Kind::Roof => "Dach",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Wall => "Wand",
            Kind::Slab => "Decke",
            Kind::Roof => "Dach",
        }
    }

    pub fn id_prefix(self) -> char {
        match self {
            Kind::Wall => 'W',
            Kind::Slab => 'S',
            Kind::Roof => 'R',
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Building {
    pub units: &'static str,
    pub storeys: Vec<Storey>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Serialize)]
pub struct Storey {
    pub name: String,
    pub file: String,
    pub elements: Vec<Element>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Element {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub layer: String,
    /// Walls only: true when one side faces outdoors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exterior: Option<bool>,
    pub thickness: f64,
    /// Walls: along the wall axis. Slabs/roofs: longest side of the bounding box in plan.
    pub length: f64,
    /// Walls: highest point above the wall bottom. Slabs/roofs: vertical extent.
    pub height: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_deg: Option<f64>,
    pub bottom_z: f64,
    pub gross_area: f64,
    pub openings_area: f64,
    pub net_area: f64,
    pub volume: f64,
    /// Outer loop of the reference face. The element extends `thickness` from
    /// this face along `normal`.
    pub outline: Vec<V3>,
    pub holes: Vec<Vec<V3>>,
    pub normal: V3,
    /// In-plane direction of the reference face (wall axis, slab/roof long side).
    pub axis: V3,
    /// Walls only: bottom centerline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<V3>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<V3>,
    pub openings: Vec<Opening>,
    /// All faces of the solid (loops of vertices), for 3D previews. Not written to JSON.
    #[serde(skip)]
    pub faces: Vec<Vec<Vec<V3>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpeningKind {
    Door,
    Window,
    /// Reaches the floor but is too low for a door (hatches, pass-throughs).
    Other,
}

impl OpeningKind {
    pub fn label(self) -> &'static str {
        match self {
            OpeningKind::Door => "Tür",
            OpeningKind::Window => "Fenster",
            OpeningKind::Other => "Öffnung",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Opening {
    pub id: String,
    pub kind: OpeningKind,
    pub width: f64,
    pub height: f64,
    /// Height of the opening bottom above the wall bottom.
    pub sill: f64,
    pub area: f64,
    /// Position along the wall axis, measured from the wall start.
    pub offset: f64,
    pub outline: Vec<V3>,
}

#[derive(Debug, Serialize)]
pub struct Skipped {
    pub file: String,
    pub layer: String,
    pub reason: String,
    pub count: usize,
}
