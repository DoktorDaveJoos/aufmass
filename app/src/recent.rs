//! The projects opened lately, newest first, kept in the app's settings folder
//! (recent.json) across starts. As in Thermkern.

use gpui::Global;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How many projects the list keeps.
const KEEP: usize = 30;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    /// The name the project had when it was last opened, shown also when the file has gone.
    pub name: String,
    /// When it was last opened, Unix seconds.
    pub opened: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    projects: Vec<Entry>,
}

#[derive(Default)]
pub struct Recent {
    /// Where the list is kept; none where the platform has no settings folder.
    file: Option<PathBuf>,
    pub entries: Vec<Entry>,
}

impl Global for Recent {}

impl Recent {
    /// The list kept in `file`; empty when there is none yet or it cannot be read.
    pub fn load(file: Option<PathBuf>) -> Recent {
        let entries = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|bytes| serde_json::from_slice::<File>(&bytes).ok())
            .map(|f| f.projects)
            .unwrap_or_default();
        Recent { file, entries }
    }

    /// Puts the project first, with its current name.
    pub fn touch(&mut self, path: &Path, name: &str, now: i64) {
        self.remove(path);
        self.entries.insert(0, Entry { path: path.to_owned(), name: name.to_owned(), opened: now });
        self.entries.truncate(KEEP);
    }

    pub fn remove(&mut self, path: &Path) {
        self.entries.retain(|e| !crate::places::same(&e.path, path));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Writes the list, beside the old one first and moved over it.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(file) = &self.file else { return Ok(()) };
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let part = file.with_extension("json.part");
        std::fs::write(&part, serde_json::to_vec_pretty(&File { projects: self.entries.clone() })?)?;
        std::fs::rename(&part, file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_first_once() {
        let dir = std::env::temp_dir().join(format!("aufmass-recent-{}", std::process::id()));
        let file = dir.join("recent.json");
        let mut recent = Recent::load(Some(file.clone()));
        assert!(recent.entries.is_empty());
        for (i, name) in ["A", "B", "C"].iter().enumerate() {
            recent.touch(&dir.join(format!("{name}.aufmass")), name, i as i64);
        }
        recent.touch(&dir.join("A.aufmass"), "A neu", 10);
        let names: Vec<_> = recent.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["A neu", "C", "B"]);
        recent.save().unwrap();
        assert_eq!(Recent::load(Some(file.clone())).entries, recent.entries);

        recent.remove(&dir.join("C.aufmass"));
        assert_eq!(recent.entries.len(), 2);
        for i in 0..40 {
            recent.touch(&dir.join(format!("{i}.aufmass")), "x", i);
        }
        assert_eq!(recent.entries.len(), KEEP);

        std::fs::write(&file, "kein json").unwrap();
        assert!(Recent::load(Some(file)).entries.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
