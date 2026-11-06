//! Where things are kept on each platform, and paths as the user reads them. As in Thermkern.

use crate::project::EXTENSION;
use std::path::{Path, PathBuf};

/// The folder of the app's own files (the recent list), shared with the remembered
/// programs: `%APPDATA%\aufmass`, elsewhere `~/.config/aufmass`.
pub fn settings() -> Option<PathBuf> {
    aufmass::cadwork::app_data()
}

/// Where new projects are suggested: Dokumente/Aufmaß, created when missing;
/// Dokumente itself, or the home folder, if that fails.
pub fn projects() -> PathBuf {
    let documents = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
    let dir = documents.join(crate::NAME);
    match std::fs::create_dir_all(&dir) {
        Ok(()) => dir,
        Err(_) => documents,
    }
}

/// A file name for a new project in `dir` that is not taken: "Neues Projekt.aufmass",
/// else "Neues Projekt 2.aufmass" and on.
pub fn free_name(dir: &Path, base: &str) -> String {
    (1..)
        .map(|n| match n {
            1 => format!("{base}.{EXTENSION}"),
            n => format!("{base} {n}.{EXTENSION}"),
        })
        .find(|name| !dir.join(name).exists())
        .unwrap_or_default()
}

/// The path with the project extension, which a save dialog may have left off.
pub fn with_extension(path: PathBuf) -> PathBuf {
    if is_project(&path) {
        return path;
    }
    let mut name = path.into_os_string();
    name.push(".");
    name.push(EXTENSION);
    PathBuf::from(name)
}

/// Whether the file is named as a project.
pub fn is_project(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// Whether two paths name the same file as the platform compares names: ignoring case on Windows and macOS.
pub fn same(a: &Path, b: &Path) -> bool {
    if cfg!(target_os = "linux") {
        a == b
    } else {
        a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
    }
}

/// The project's name from its file name.
pub fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A name for a project made from plans: the folder they are in, or the folder chosen.
pub fn name_for_plans(plans: &[PathBuf]) -> String {
    let Some(first) = plans.first() else { return String::new() };
    let folder = if first.is_dir() { Some(first.as_path()) } else { first.parent() };
    folder.and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The folder a file is in, as the user knows it: "Dokumente/Aufmaß" (with the platform's
/// separator), "~/…" for the rest of the home folder on macOS, the full path otherwise.
pub fn folder(path: &Path) -> String {
    let Some(parent) = path.parent() else { return String::new() };
    let sep = std::path::MAIN_SEPARATOR_STR;
    let under = |base: Option<PathBuf>, label: &str| {
        let rest = parent.strip_prefix(base?).ok()?;
        Some(rest.components().fold(label.to_string(), |s, c| format!("{s}{sep}{}", c.as_os_str().to_string_lossy())))
    };
    under(dirs::document_dir(), "Dokumente")
        .or_else(|| if cfg!(target_os = "macos") { under(dirs::home_dir(), "~") } else { None })
        .unwrap_or_else(|| parent.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_the_extension_once() {
        assert_eq!(with_extension(PathBuf::from("/a/Haus")), PathBuf::from("/a/Haus.aufmass"));
        assert_eq!(with_extension(PathBuf::from("/a/Haus.AUFMASS")), PathBuf::from("/a/Haus.AUFMASS"));
        assert_eq!(with_extension(PathBuf::from("/a/Haus 1.5")), PathBuf::from("/a/Haus 1.5.aufmass"));
    }

    #[test]
    fn names_a_project_after_its_plans() {
        let dir = std::env::temp_dir().join(format!("aufmass-places-{}", std::process::id())).join("Haus Müller");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(name_for_plans(std::slice::from_ref(&dir)), "Haus Müller");
        assert_eq!(name_for_plans(&[dir.join("eg.dwg")]), "Haus Müller");
        assert_eq!(name_for_plans(&[]), "");
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn shows_documents_by_name() {
        let Some(docs) = dirs::document_dir() else { return };
        let sep = std::path::MAIN_SEPARATOR_STR;
        assert_eq!(folder(&docs.join("Aufmaß").join("Haus.aufmass")), format!("Dokumente{sep}Aufmaß"));
        assert_eq!(folder(&docs.join("Haus.aufmass")), "Dokumente");
    }
}
