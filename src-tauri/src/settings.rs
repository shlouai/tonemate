//! What the user can change about the bar, and where it is kept.
//!
//! The value lives in Rust rather than in the webview's own storage because two
//! windows need it: the settings window writes it, and the bar reads it at
//! startup and again whenever it changes. One owner means there is no question
//! of which webview's copy is current — the frontend asks and listens, the same
//! shape `submit` already has.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// The colour the app ships with, and what it falls back to. Named rather than
/// spelled as a colour: what the name paints is `src/palette.css`'s business.
const DEFAULT_ACCENT: &str = "graphite";

/// Sits in the app's config directory, next to nothing else for now.
const FILE: &str = "settings.json";

/// A JSON object rather than a bare string, so the next setting can join it
/// without changing the format of what is already on disk.
#[derive(Serialize, Deserialize)]
struct Stored {
    accent: String,
}

/// The accent the bar should be painted in.
#[tauri::command]
pub fn accent(app: AppHandle) -> String {
    match file(&app) {
        Some(file) => read(&file),
        None => DEFAULT_ACCENT.to_string(),
    }
}

/// Records the accent and tells the bar to repaint, so the choice takes effect
/// while the settings window is still open rather than at the next launch.
///
/// The value is stored as it arrives: only the swatches in the settings window
/// send one, and an accent `src/palette.css` doesn't define simply renders as
/// the default — so there is nothing here for Rust to have an opinion about.
#[tauri::command]
pub fn set_accent(app: AppHandle, accent: String) -> Result<(), String> {
    let file = file(&app).ok_or_else(|| "no config directory".to_string())?;
    write(&file, &accent)?;
    println!("[tonemate] accent -> {accent}");
    // Emitted to the bar alone: it is the only window that is painted with this,
    // and the settings window already knows — it is what asked.
    let _ = app.emit_to(crate::MAIN_WINDOW, "settings:accent", &accent);
    Ok(())
}

fn file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join(FILE))
}

/// The accent in `file`, or the default when there is nothing usable there. A
/// missing file is the first run and a corrupt one is a bug elsewhere; neither
/// is worth refusing to draw the bar over, when the fallback is the colour it
/// would have had anyway.
fn read(file: &Path) -> String {
    fs::read_to_string(file)
        .ok()
        .and_then(|json| serde_json::from_str::<Stored>(&json).ok())
        .map(|stored| stored.accent)
        .unwrap_or_else(|| DEFAULT_ACCENT.to_string())
}

fn write(file: &Path, accent: &str) -> Result<(), String> {
    let stored = Stored {
        accent: accent.to_string(),
    };
    let json = serde_json::to_string(&stored).map_err(|err| err.to_string())?;
    // The config directory is created lazily by whatever first writes to it, and
    // this is the only thing in the app that ever does.
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    fs::write(file, json).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path in the temp directory that nothing else in this run will use.
    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("tonemate-{}-{}.json", std::process::id(), name))
    }

    #[test]
    fn missing_file_reads_as_the_default() {
        let file = scratch("missing");
        let _ = fs::remove_file(&file);
        assert_eq!(read(&file), DEFAULT_ACCENT);
    }

    #[test]
    fn a_written_accent_reads_back() {
        let file = scratch("round-trip");
        write(&file, "indigo").unwrap();
        assert_eq!(read(&file), "indigo");
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn unreadable_contents_read_as_the_default() {
        let file = scratch("corrupt");
        fs::write(&file, "{ not json").unwrap();
        assert_eq!(read(&file), DEFAULT_ACCENT);
        let _ = fs::remove_file(&file);
    }
}
