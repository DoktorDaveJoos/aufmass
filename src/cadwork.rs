//! Hand-over to cadwork 3d: the import plugin (`cadwork/Aufmass_Import`) and
//! the pointer file that tells it which building.json to load.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const PLUGIN_NAME: &str = "Aufmass_Import";
const PLUGIN: &str = include_str!("../cadwork/Aufmass_Import/Aufmass_Import.py");

/// `%APPDATA%\aufmass` (elsewhere `~/.config/aufmass`), shared with the plugin.
pub fn app_data() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("aufmass"))
}

/// Records `building_json` as the latest result, which the plugin offers first.
pub fn remember_output(building_json: &Path) -> Result<()> {
    let dir = app_data().context("no APPDATA/HOME")?;
    std::fs::create_dir_all(&dir)?;
    let abs = std::path::absolute(building_json)?;
    std::fs::write(dir.join("letzte-auswertung.txt"), abs.display().to_string())?;
    Ok(())
}

/// Copies the plugin into the plugin folder of every cadwork user profile
/// (`%APPDATA%\cadwork\userprofil_*\3d\API.x64`) and returns those folders.
/// Empty when no cadwork profile is found.
pub fn install_plugin() -> Vec<PathBuf> {
    let Some(cadwork) = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("cadwork")) else {
        return vec![];
    };
    let Ok(profiles) = std::fs::read_dir(&cadwork) else { return vec![] };
    profiles
        .flatten()
        .filter(|p| p.file_name().to_string_lossy().starts_with("userprofil"))
        .map(|p| p.path().join("3d").join("API.x64"))
        .filter(|api| api.is_dir())
        .filter_map(|api| write_plugin(&api).ok().map(|()| api))
        .collect()
}

/// Writes the plugin folder into `api_dir`.
pub fn write_plugin(api_dir: &Path) -> Result<()> {
    let dir = api_dir.join(PLUGIN_NAME);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{PLUGIN_NAME}.py"));
    if std::fs::read_to_string(&file).ok().as_deref() != Some(PLUGIN) {
        std::fs::write(&file, PLUGIN).with_context(|| format!("{} konnte nicht geschrieben werden", file.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_is_written_into_its_own_folder() {
        let api = std::env::temp_dir().join(format!("aufmass-api-{}", std::process::id()));
        write_plugin(&api).unwrap();
        let file = api.join(PLUGIN_NAME).join(format!("{PLUGIN_NAME}.py"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), PLUGIN);
        std::fs::remove_dir_all(&api).unwrap();
    }
}
