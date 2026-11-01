//! Updates from GitHub Releases, shared by the CLI and the app.
//!
//! A release (tag `vX.Y.Z`) carries one zip per platform, signed by CI with
//! the zipsign key whose public half is `packaging/update-key.pub`. The
//! program asks GitHub for the latest release, downloads the zip for its
//! platform, checks the signature and replaces the files next to itself.
//! aufmass.toml stays as it is: it holds the user's settings.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;
use ureq::tls::{TlsConfig, TlsProvider};

/// `owner/name` of the repository with the releases; must be public, GitHub
/// hides releases of private repositories. `AUFMASS_RELEASES` at build time
/// points it elsewhere, e.g. at a public repository holding only releases.
const REPO: &str = match option_env!("AUFMASS_RELEASES") {
    Some(r) => r,
    None => "DoktorDaveJoos/aufmass",
};

const PUBLIC_KEY: &[u8; 32] = include_bytes!("../packaging/update-key.pub");

/// The release asset for this platform; CI names the zips the same way.
pub const ASSET: &str = if cfg!(windows) {
    "aufmass-windows-x64.zip"
} else if cfg!(target_os = "macos") {
    "aufmass-macos-arm64.zip"
} else {
    ""
};

const CONFIG_FILE: &str = "aufmass.toml";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Release {
    pub version: String,
    url: String,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

/// The latest release, if it is newer than this program and has a zip for
/// this platform.
pub fn check() -> Result<Option<Release>> {
    if ASSET.is_empty() {
        return Ok(None);
    }
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    // For testing against a local server; release builds always ask GitHub.
    #[cfg(debug_assertions)]
    let url = std::env::var("AUFMASS_RELEASES_API").unwrap_or(url);
    let release: GitHubRelease = match agent(Some(Duration::from_secs(20)))
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(mut r) => r.body_mut().read_json().context("Antwort von GitHub nicht lesbar")?,
        // No release yet, or the repository is private.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e).context("GitHub nicht erreichbar"),
    };
    let version = release.tag_name.trim_start_matches('v').to_string();
    if !newer(&version, VERSION) {
        return Ok(None);
    }
    Ok(release.assets.into_iter().find(|a| a.name == ASSET).map(|a| Release { version, url: a.browser_download_url }))
}

/// Downloads, verifies and installs `release` next to the running program.
/// Returns the program to start for the new version.
pub fn install(release: &Release) -> Result<PathBuf> {
    let zip = agent(None)
        .get(&release.url)
        .call()
        .and_then(|mut r| r.body_mut().with_config().limit(500 * 1024 * 1024).read_to_vec())
        .context("Update konnte nicht geladen werden")?;
    verify(&zip, &release.version)?;
    let exe = std::env::current_exe().context("Programmordner nicht gefunden")?;
    let dir = exe.parent().context("Programmordner nicht gefunden")?;
    let staged = stage(&zip, dir)?;
    let result = if staged.iter().any(|(_, t)| t.file_name() == exe.file_name()) {
        replace(&staged, &exe)
    } else {
        Err(anyhow::anyhow!("Update enthält dieses Programm nicht"))
    };
    if let Err(e) = result {
        for (tmp, _) in &staged {
            fs::remove_file(tmp).ok();
        }
        return Err(e);
    }
    Ok(exe)
}

/// Checks the zipsign signature. The context binds the zip to its platform
/// and version, so an older signed zip can't be passed off as a new release.
fn verify(zip: &[u8], version: &str) -> Result<()> {
    let key = zipsign_api::VerifyingKey::from_bytes(PUBLIC_KEY).context("Update-Schlüssel ungültig")?;
    let context = format!("{ASSET} {version}");
    zipsign_api::verify::verify_zip(&mut Cursor::new(zip), &[key], Some(context.as_bytes()))
        .context("Update nicht vertrauenswürdig: die Signatur passt nicht")?;
    Ok(())
}

/// Unpacks the zip into `<name>.update` files next to the targets, so a
/// broken download never leaves half an update behind.
fn stage(zip: &[u8], dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip)).context("Update-Datei beschädigt")?;
    let mut staged = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("Update-Datei beschädigt")?;
        // CI zips the files flat; anything in a folder isn't ours.
        let Some(name) = entry.enclosed_name().filter(|p| p.components().count() == 1) else { continue };
        let target = dir.join(&name);
        if entry.is_dir() || (name == Path::new(CONFIG_FILE) && target.exists()) {
            continue;
        }
        let mut tmp = target.clone().into_os_string();
        tmp.push(".update");
        let tmp = PathBuf::from(tmp);
        let written = fs::File::create(&tmp).and_then(|mut f| std::io::copy(&mut entry, &mut f));
        if let Err(e) = written {
            for (t, _) in &staged {
                fs::remove_file(t).ok();
            }
            fs::remove_file(&tmp).ok();
            return Err(e).with_context(|| format!("Update konnte nicht in {} gespeichert werden", dir.display()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if entry.unix_mode().is_some_and(|m| m & 0o111 != 0) || name.extension().is_none() {
                fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
            }
        }
        staged.push((tmp, target));
    }
    Ok(staged)
}

/// Moves the staged files into place, the running program last: Windows
/// can't overwrite it, self_replace moves it aside first.
fn replace(staged: &[(PathBuf, PathBuf)], exe: &Path) -> Result<()> {
    let is_self = |t: &PathBuf| t.file_name() == exe.file_name();
    for (tmp, target) in staged.iter().filter(|(_, t)| !is_self(t)) {
        fs::rename(tmp, target).with_context(|| {
            format!("{} konnte nicht ersetzt werden, ist es noch geöffnet?", target.file_name().unwrap_or_default().to_string_lossy())
        })?;
    }
    if let Some((tmp, _)) = staged.iter().find(|(_, t)| is_self(t)) {
        self_replace::self_replace(tmp).context("Programm konnte nicht ersetzt werden")?;
        fs::remove_file(tmp).ok();
    }
    Ok(())
}

fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().provider(TlsProvider::NativeTls).build())
        .user_agent(concat!("aufmass/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(timeout)
        .build()
        .into()
}

/// Compares `x.y.z` versions; anything else is never newer.
fn newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|n| n.parse::<u64>().ok()).collect::<Option<Vec<_>>>();
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.10.0", "0.9.3"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.2.0-beta", "0.1.0"));
    }
}
