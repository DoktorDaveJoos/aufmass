//! Crash and error reports to Sentry, shared by the CLI and the app.
//!
//! Off unless a DSN is set: `SENTRY_DSN` at build time (CI secret) or in the
//! environment, where an empty value turns reporting off. Plans never leave
//! the PC. Reports carry error messages, file names and counts; plan folders
//! (often named after the client) and the home folder are replaced by
//! placeholders, and the computer name is dropped.

use crate::FileReport;
use sentry::protocol::{DebugImage, Event, Level, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Folders of the current conversion, scrubbed from every report.
static FOLDERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Starts reporting; keep the guard alive until the program ends.
/// `program` tags the reports ("cli" or "app").
pub fn init(program: &'static str) -> Option<sentry::ClientInitGuard> {
    let dsn = std::env::var("SENTRY_DSN").ok().or(option_env!("SENTRY_DSN").map(String::from)).filter(|d| !d.is_empty())?;
    let mut options = sentry::ClientOptions::default();
    options.release = Some(concat!("aufmass@", env!("CARGO_PKG_VERSION")).into());
    options.environment = Some(if cfg!(debug_assertions) { "development" } else { "production" }.into());
    options.send_default_pii = false;
    options.before_send = Some(Arc::new(|mut e| {
        scrub_event(&mut e);
        Some(e)
    }));
    let guard = sentry::init((dsn, options));
    sentry::configure_scope(|s| s.set_tag("program", program));
    Some(guard)
}

/// Remembers the plan folders of this conversion, so reports leave them out.
pub fn set_folders(inputs: &[PathBuf], out: &Path) {
    let mut folders = Vec::new();
    for p in inputs.iter().map(PathBuf::as_path).chain([out]) {
        let dir = if p.is_dir() || p == out { p } else { p.parent().unwrap_or(p) };
        folders.push(dir.display().to_string());
        if let Ok(abs) = std::path::absolute(dir) {
            folders.push(abs.display().to_string());
        }
    }
    // Longest first, so a folder is replaced before the folder it sits in.
    folders.retain(|f| f.len() > 1);
    folders.sort_by_key(|f| std::cmp::Reverse(f.len()));
    folders.dedup();
    *FOLDERS.lock().unwrap_or_else(|e| e.into_inner()) = folders;
}

/// Reports a conversion that went wrong. `failure` is why nothing usable came
/// out (no 3D model, results not saved); without one, only files or 3D solids
/// that could not be read are reported. A clean conversion sends nothing.
pub fn conversion(files: &[FileReport], failure: Option<&anyhow::Error>) {
    let unreadable_files = files.iter().filter(|f| f.error.is_some()).count();
    let unreadable_solids: usize = files.iter().map(|f| f.unreadable).sum();
    // Grouped by the outermost error message, without the causes below it.
    let (message, group) = match failure {
        Some(e) => (format!("{e:#}"), scrub(&e.to_string())),
        None if unreadable_files + unreadable_solids > 0 => ("Dateien oder 3D-Körper nicht lesbar".to_string(), "unreadable".to_string()),
        None => return,
    };
    let mut event = Event {
        level: if failure.is_some() { Level::Error } else { Level::Warning },
        message: Some(message),
        fingerprint: vec!["conversion".into(), group.into()].into(),
        ..Default::default()
    };
    event.tags.insert("files".into(), files.len().to_string());
    event.tags.insert("unreadable_files".into(), unreadable_files.to_string());
    event.tags.insert("unreadable_solids".into(), unreadable_solids.to_string());
    event.extra.insert("files".into(), Value::Array(files.iter().map(|f| Value::String(f.line())).collect()));
    sentry::capture_event(event);
}

fn scrub_event(e: &mut Event<'static>) {
    e.server_name = None;
    e.user = None;
    if let Some(m) = &mut e.message {
        *m = scrub(m);
    }
    if let Some(l) = &mut e.logentry {
        l.message = scrub(&l.message);
    }
    for x in &mut e.exception.values {
        if let Some(v) = &mut x.value {
            *v = scrub(v);
        }
    }
    for v in e.extra.values_mut() {
        scrub_value(v);
    }
    // Where the program is installed, e.g. under C:\Users\<name>.
    for image in &mut e.debug_meta.to_mut().images {
        if let DebugImage::Symbolic(i) = image {
            i.name = scrub(&i.name);
        }
    }
}

fn scrub_value(v: &mut Value) {
    match v {
        Value::String(s) => *s = scrub(s),
        Value::Array(a) => a.iter_mut().for_each(scrub_value),
        Value::Object(o) => o.values_mut().for_each(scrub_value),
        _ => {}
    }
}

fn scrub(text: &str) -> String {
    let mut t = text.to_string();
    for f in FOLDERS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        t = t.replace(f.as_str(), "<Planordner>");
    }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    if home.len() > 1 {
        t = t.replace(&home, "~");
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_folders_are_scrubbed() {
        let plans = std::env::temp_dir().join("Projekte").join("Haus Müller");
        set_folders(&[plans.join("eg.dwg")], &plans.join("aufmass"));
        let text = format!("{} gesperrt, {} fehlt", plans.join("aufmass").join("mengen.xlsx").display(), plans.join("og.dwg").display());
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(scrub(&text), format!("<Planordner>{sep}mengen.xlsx gesperrt, <Planordner>{sep}og.dwg fehlt"));
    }
}
