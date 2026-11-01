//! Finds PAPA and cadwork 3d for "In PAPA öffnen" and "In cadwork öffnen":
//! the path in aufmass.toml, else the program picked in the app before, else
//! a search of the installed programs (Windows) and their usual folders.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Program {
    Papa,
    Cadwork,
}

impl Program {
    pub fn name(self) -> &'static str {
        match self {
            Program::Papa => "PAPA",
            Program::Cadwork => "cadwork",
        }
    }

    /// The program file, as the user looks for it in the file dialog.
    pub fn exe_hint(self) -> &'static str {
        match self {
            Program::Papa => "PAPA.exe",
            Program::Cadwork => "ci.exe",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Program::Papa => "papa",
            Program::Cadwork => "cadwork",
        }
    }

    fn is_exe(self, file: &Path) -> bool {
        let name = file.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        match self {
            Program::Papa => name.ends_with(".exe") && name.contains("papa"),
            Program::Cadwork => name == "ci.exe",
        }
    }
}

/// The program to start; `configured` is the path from aufmass.toml.
pub fn find(program: Program, configured: Option<&Path>) -> Option<PathBuf> {
    configured
        .map(Path::to_path_buf)
        .into_iter()
        .chain(remembered(program))
        .chain(installed(program))
        .chain(in_usual_folders(program))
        .find(|p| p.is_file())
}

/// Keeps the program the user picked, for the next time; aufmass.toml
/// still comes first.
pub fn remember(program: Program, exe: &Path) -> Result<()> {
    let file = store().context("Kein Ordner für Einstellungen gefunden")?;
    remember_in(&file, program, exe).with_context(|| format!("{} konnte nicht gespeichert werden", file.display()))
}

/// `%APPDATA%\aufmass\programme.toml`.
fn store() -> Option<PathBuf> {
    crate::cadwork::app_data().map(|d| d.join("programme.toml"))
}

fn remembered(program: Program) -> Option<PathBuf> {
    remembered_in(&store()?, program)
}

fn remembered_in(file: &Path, program: Program) -> Option<PathBuf> {
    let mut all: BTreeMap<String, PathBuf> = toml::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    all.remove(program.key())
}

fn remember_in(file: &Path, program: Program, exe: &Path) -> Result<()> {
    let mut all: BTreeMap<String, PathBuf> =
        std::fs::read_to_string(file).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();
    all.insert(program.key().into(), std::path::absolute(exe)?);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, toml::to_string(&all)?)?;
    Ok(())
}

/// Programs listed under "Apps & Features", from their uninstall entries.
#[cfg(windows)]
fn installed(program: Program) -> Vec<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
    use winreg::RegKey;
    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    let hives = [
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY),
        (HKEY_CURRENT_USER, KEY_READ),
    ];
    let mut found = Vec::new();
    for (hive, flags) in hives {
        let Ok(list) = RegKey::predef(hive).open_subkey_with_flags(UNINSTALL, flags) else { continue };
        for name in list.enum_keys().flatten() {
            let Ok(entry) = list.open_subkey_with_flags(&name, flags) else { continue };
            let Ok(display) = entry.get_value::<String, _>("DisplayName") else { continue };
            if !display.to_lowercase().contains(program.key()) {
                continue;
            }
            // Usually the program itself, e.g. "C:\PAPA\PAPA.exe",0.
            if let Ok(icon) = entry.get_value::<String, _>("DisplayIcon") {
                let icon = icon.trim().trim_matches('"');
                let icon = match icon.rsplit_once(',') {
                    Some((path, index)) if index.trim().parse::<i32>().is_ok() => path.trim().trim_matches('"'),
                    _ => icon,
                };
                let icon = PathBuf::from(icon);
                let file = icon.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                if file.ends_with(".exe") && !file.starts_with("unins") {
                    found.push(icon);
                }
            }
            if let Ok(dir) = entry.get_value::<String, _>("InstallLocation") {
                found.extend(exes_in(Path::new(dir.trim().trim_matches('"')), program, 2));
            }
        }
    }
    found.sort_by_key(|p| !program.is_exe(p));
    found
}

#[cfg(not(windows))]
fn installed(_: Program) -> Vec<PathBuf> {
    Vec::new()
}

/// cadwork in `C:\cadwork\v30`, PAPA in a folder named after it under
/// Program Files.
fn in_usual_folders(program: Program) -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    // "C:" is the current folder on drive C, "C:\" its root.
    let drive = std::env::var("SystemDrive").ok().map(|d| PathBuf::from(format!("{d}\\")));
    match program {
        Program::Cadwork => {
            [drive, var("ProgramFiles")].into_iter().flatten().filter_map(|root| newest_cadwork(&root.join("cadwork"))).collect()
        }
        Program::Papa => [var("ProgramFiles"), var("ProgramFiles(x86)"), drive]
            .into_iter()
            .flatten()
            .flat_map(|root| subdirs(&root))
            .filter(|d| d.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("papa")))
            .flat_map(|d| exes_in(&d, program, 2))
            .collect(),
    }
}

/// `ci.exe` of the highest version folder (`v30` before `v29`).
fn newest_cadwork(root: &Path) -> Option<PathBuf> {
    let version = |d: &Path| d.file_name()?.to_str()?.strip_prefix(['v', 'V'])?.parse::<u32>().ok();
    let mut versions: Vec<(u32, PathBuf)> = subdirs(root).into_iter().filter_map(|d| Some((version(&d)?, d))).collect();
    versions.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
    versions.into_iter().map(|(_, d)| d.join("ci.exe")).find(|p| p.is_file())
}

/// Program files of `program` in `dir` and up to `depth` folders below.
fn exes_in(dir: &Path, program: Program, depth: usize) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    let mut found: Vec<PathBuf> = paths.iter().filter(|p| p.is_file() && program.is_exe(p)).cloned().collect();
    if depth > 0 {
        found.extend(paths.iter().filter(|p| p.is_dir()).flat_map(|d| exes_in(d, program, depth - 1)));
    }
    found
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aufmass-programs-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn newest_cadwork_version_wins() {
        let root = temp("cadwork");
        for v in ["v4", "v29", "v30", "vorlagen"] {
            touch(&root.join(v).join("ci.exe"));
        }
        assert_eq!(newest_cadwork(&root), Some(root.join("v30").join("ci.exe")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn papa_found_below_its_folder() {
        let root = temp("papa");
        touch(&root.join("bin").join("PAPA.exe"));
        touch(&root.join("bin").join("papa.ini"));
        touch(&root.join("unins000.exe"));
        assert_eq!(exes_in(&root, Program::Papa, 2), vec![root.join("bin").join("PAPA.exe")]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn picked_programs_are_kept_apart() {
        let root = temp("store");
        let file = root.join("programme.toml");
        // Absolute on every platform, e.g. C:\x\PAPA.exe on Windows.
        let papa = root.join("x").join("PAPA.exe");
        let cadwork = root.join("y").join("ci.exe");
        remember_in(&file, Program::Papa, &papa).unwrap();
        remember_in(&file, Program::Cadwork, &cadwork).unwrap();
        assert_eq!(remembered_in(&file, Program::Papa), Some(papa));
        assert_eq!(remembered_in(&file, Program::Cadwork), Some(cadwork));
        std::fs::remove_dir_all(&root).ok();
    }
}
