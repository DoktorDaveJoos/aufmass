//! The project file, `<Name>.aufmass`: which plans a building is evaluated
//! from, kept wherever it is saved (Dokumente/Aufmaß suggested). The results
//! stay in the "aufmass" folder beside the plans, as without a project; opening
//! a project evaluates its plans again, so it always shows their current state.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const EXTENSION: &str = "aufmass";
/// The file's format; a newer one is refused with a hint to update.
const FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Marks the file as an Aufmaß project, with its format.
    aufmass_project: u32,
    /// When it was created, Unix seconds, and by which version.
    pub created: i64,
    pub app: String,
    /// The plan folders or files evaluated last, as chosen.
    pub plans: Vec<PathBuf>,
}

impl Project {
    pub fn new(app: &str, now: i64) -> Project {
        Project { aufmass_project: FORMAT, created: now, app: app.to_owned(), plans: Vec::new() }
    }

    /// The project in `path`; the error says in German why it cannot be opened.
    pub fn open(path: &Path) -> Result<Project, String> {
        let bytes = std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "Die Datei gibt es nicht mehr.".to_string(),
            _ => format!("Die Datei lässt sich nicht lesen: {e}"),
        })?;
        let project: Project = serde_json::from_slice(&bytes).map_err(|_| "Das ist keine Aufmaß-Projektdatei.".to_string())?;
        if project.aufmass_project > FORMAT {
            return Err("Sie stammt aus einer neueren Version von Aufmaß; bitte Aufmaß aktualisieren.".into());
        }
        Ok(project)
    }

    /// Writes the project, beside the old file first and moved over it.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let write = || -> std::io::Result<()> {
            let mut part = path.as_os_str().to_owned();
            part.push(".part");
            let part = PathBuf::from(part);
            std::fs::write(&part, serde_json::to_vec_pretty(self)?)?;
            std::fs::rename(&part, path)
        };
        write().map_err(|e| format!("Das Projekt lässt sich nicht speichern: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aufmass-project-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn keeps_its_plans() {
        let path = temp("Haus.aufmass");
        let mut project = Project::new("0.4.0", 1_800_000_000);
        project.plans = vec![PathBuf::from("/Pläne/Planeingang_17.09.2026")];
        project.save(&path).unwrap();
        assert_eq!(Project::open(&path).unwrap(), project);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn refuses_other_files_and_newer_ones() {
        let path = temp("Fremd.aufmass");
        std::fs::write(&path, r#"{"projects": []}"#).unwrap();
        assert_eq!(Project::open(&path).unwrap_err(), "Das ist keine Aufmaß-Projektdatei.");
        std::fs::write(&path, r#"{"aufmass_project": 2, "created": 0, "app": "9.0.0", "plans": []}"#).unwrap();
        assert!(Project::open(&path).unwrap_err().contains("neueren Version"));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(Project::open(&path).unwrap_err(), "Die Datei gibt es nicht mehr.");
    }
}
