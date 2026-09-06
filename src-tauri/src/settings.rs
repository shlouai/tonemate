//! What the user can change, and where it is kept.
//!
//! The values live in Rust rather than in the webview's own storage because two
//! windows need them: the settings window writes them, and the bar reads the
//! accent at startup and again whenever it changes. One owner means there is no
//! question of which webview's copy is current — the frontend asks and listens,
//! the same shape `submit` already has.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// The colour the app ships with, and what it falls back to. Named rather than
/// spelled as a colour: what the name paints is `src/palette.css`'s business.
const DEFAULT_ACCENT: &str = "graphite";

/// Bedrock is the default because it needs nothing typed in: an AWS profile in
/// the environment is the setup this app started with.
const DEFAULT_PROVIDER: &str = "bedrock";

/// Sits in the app's config directory, next to nothing else for now.
const FILE: &str = "settings.json";

/// `#[serde(default)]` is the migration guarantee: a file written before
/// providers existed holds only `accent`, and must keep working. Empty strings
/// are then read as "not set" and resolved to the defaults by `read`, so no
/// field needs a custom deserializer.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Stored {
    accent: String,
    /// `"bedrock"`, `"kimi"`, `"deepseek"`, or `"qwen"`. A string rather than an
    /// enum so a name written by a later version survives a round trip through
    /// this one instead of taking the whole file down with it.
    provider: String,
    /// Empty means unset, which is what sends translation back to Bedrock.
    kimi_api_key: String,
    deepseek_api_key: String,
    qwen_api_key: String,
    /// The AWS profile Bedrock uses. Empty means "use AWS's own default chain"
    /// (`AWS_PROFILE`, or the `default` profile in `~/.aws/config`). Not a
    /// secret, so unlike the keys it is sent to the window in full.
    aws_profile: String,
}

/// The accent the bar should be painted in.
#[tauri::command]
pub fn accent(app: AppHandle) -> String {
    load(&app).accent
}

/// Records the accent and tells the bar to repaint, so the choice takes effect
/// while the settings window is still open rather than at the next launch.
///
/// The value is stored as it arrives: only the swatches in the settings window
/// send one, and an accent `src/palette.css` doesn't define simply renders as
/// the default — so there is nothing here for Rust to have an opinion about.
#[tauri::command]
pub fn set_accent(app: AppHandle, accent: String) -> Result<(), String> {
    update(&app, |stored| stored.accent = accent.clone())?;
    println!("[tonemate] accent -> {accent}");
    // Emitted to the bar alone: it is the only window that is painted with this,
    // and the settings window already knows — it is what asked.
    let _ = app.emit_to(crate::MAIN_WINDOW, "settings:accent", &accent);
    Ok(())
}

/// What the settings window needs to draw the translation-service panel.
///
/// Deliberately not the key itself: the window has no use for it — it shows a
/// placeholder, not the value — and a boolean is one less place a live
/// credential has to travel through.
#[derive(Serialize)]
pub struct ProviderConfig {
    pub provider: String,
    pub kimi_key_set: bool,
    pub deepseek_key_set: bool,
    pub qwen_key_set: bool,
    pub aws_profile: String,
}

#[tauri::command]
pub fn provider_config(app: AppHandle) -> ProviderConfig {
    let stored = load(&app);
    ProviderConfig {
        provider: stored.provider,
        kimi_key_set: !stored.kimi_api_key.is_empty(),
        deepseek_key_set: !stored.deepseek_api_key.is_empty(),
        qwen_key_set: !stored.qwen_api_key.is_empty(),
        aws_profile: stored.aws_profile,
    }
}

#[tauri::command]
pub fn set_provider(app: AppHandle, provider: String) -> Result<(), String> {
    update(&app, |stored| stored.provider = provider.clone())?;
    println!("[tonemate] provider -> {provider}");

    // Spawned rather than awaited so the settings window does not block on a
    // network round trip. A bad key or wrong host appears in the log at save time
    // rather than on the first translation, which is where the answer belongs
    // since this window shows no validation verdict.
    let warm_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let chosen = crate::provider::Provider::from_settings(&warm_handle);
        match crate::provider::warm(&chosen).await {
            Ok(()) => println!("[tonemate] {} warm", chosen.label()),
            Err(err) => eprintln!("[tonemate] {} warmup failed: {err}", chosen.label()),
        }
    });

    Ok(())
}

/// The AWS profile Bedrock should use. An empty string clears it back to AWS's
/// own default chain. Like `set_provider`, the warm-up is spawned so the window
/// does not block on the credential resolution a profile change can trigger.
#[tauri::command]
pub fn set_aws_profile(app: AppHandle, profile: String) -> Result<(), String> {
    let profile = profile.trim().to_string();
    update(&app, |stored| stored.aws_profile = profile.clone())?;
    println!(
        "[tonemate] aws profile -> {}",
        if profile.is_empty() { "(default)" } else { &profile }
    );

    let warm_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let chosen = crate::provider::Provider::from_settings(&warm_handle);
        match crate::provider::warm(&chosen).await {
            Ok(()) => println!("[tonemate] {} warm", chosen.label()),
            Err(err) => eprintln!("[tonemate] {} warmup failed: {err}", chosen.label()),
        }
    });

    Ok(())
}

/// An empty `key` clears the stored one. The settings window only sends empty
/// from its clear button — an empty field on blur means "unchanged" there — but
/// the command has one meaning regardless of who calls it.
#[tauri::command]
pub fn set_kimi_api_key(app: AppHandle, key: String) -> Result<(), String> {
    let key = key.trim().to_string();
    let set = !key.is_empty();
    update(&app, |stored| stored.kimi_api_key = key.clone())?;
    // Whether, never what: this line goes to the terminal the app was launched
    // from, and to any log that terminal is piped into.
    println!(
        "[tonemate] kimi api key -> {}",
        if set { "set" } else { "cleared" }
    );

    // Spawned rather than awaited so the settings window does not block on a
    // network round trip. A bad key or wrong host appears in the log at save time
    // rather than on the first translation, which is where the answer belongs
    // since this window shows no validation verdict.
    let warm_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let chosen = crate::provider::Provider::from_settings(&warm_handle);
        match crate::provider::warm(&chosen).await {
            Ok(()) => println!("[tonemate] {} warm", chosen.label()),
            Err(err) => eprintln!("[tonemate] {} warmup failed: {err}", chosen.label()),
        }
    });

    Ok(())
}

/// The DeepSeek twin of `set_kimi_api_key`, down to the empty-clears semantics
/// and the spawned warm-up. Kept separate rather than generalised: the names
/// are read as "Kimi's key" and "DeepSeek's key" in the settings window, and a
/// single parameterised command would just move that naming somewhere less
/// obvious.
#[tauri::command]
pub fn set_deepseek_api_key(app: AppHandle, key: String) -> Result<(), String> {
    let key = key.trim().to_string();
    let set = !key.is_empty();
    update(&app, |stored| stored.deepseek_api_key = key.clone())?;
    println!(
        "[tonemate] deepseek api key -> {}",
        if set { "set" } else { "cleared" }
    );

    let warm_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let chosen = crate::provider::Provider::from_settings(&warm_handle);
        match crate::provider::warm(&chosen).await {
            Ok(()) => println!("[tonemate] {} warm", chosen.label()),
            Err(err) => eprintln!("[tonemate] {} warmup failed: {err}", chosen.label()),
        }
    });

    Ok(())
}

/// The Qwen twin of `set_kimi_api_key`, same shape as the DeepSeek one.
#[tauri::command]
pub fn set_qwen_api_key(app: AppHandle, key: String) -> Result<(), String> {
    let key = key.trim().to_string();
    let set = !key.is_empty();
    update(&app, |stored| stored.qwen_api_key = key.clone())?;
    println!(
        "[tonemate] qwen api key -> {}",
        if set { "set" } else { "cleared" }
    );

    let warm_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let chosen = crate::provider::Provider::from_settings(&warm_handle);
        match crate::provider::warm(&chosen).await {
            Ok(()) => println!("[tonemate] {} warm", chosen.label()),
            Err(err) => eprintln!("[tonemate] {} warmup failed: {err}", chosen.label()),
        }
    });

    Ok(())
}

/// The chosen provider's name, for `Provider::from_settings`.
pub fn provider_name(app: &AppHandle) -> String {
    load(app).provider
}

/// The stored Kimi key, or an empty string when there is none.
pub fn kimi_api_key(app: &AppHandle) -> String {
    load(app).kimi_api_key
}

/// The stored DeepSeek key, or an empty string when there is none.
pub fn deepseek_api_key(app: &AppHandle) -> String {
    load(app).deepseek_api_key
}

/// The stored Qwen key, or an empty string when there is none.
pub fn qwen_api_key(app: &AppHandle) -> String {
    load(app).qwen_api_key
}

/// The stored AWS profile, or an empty string when there is none.
pub fn aws_profile(app: &AppHandle) -> String {
    load(app).aws_profile
}

fn file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join(FILE))
}

/// Everything stored, with defaults filled in. A missing config directory is
/// treated the same way a missing file is — as a first run.
fn load(app: &AppHandle) -> Stored {
    match file(app) {
        Some(file) => read(&file),
        None => defaults(),
    }
}

/// Read, hand to `edit`, write back. Read-modify-write rather than a
/// field-at-a-time writer, so setting one value cannot drop another.
fn update(app: &AppHandle, edit: impl FnOnce(&mut Stored)) -> Result<(), String> {
    let file = file(app).ok_or_else(|| "no config directory".to_string())?;
    let mut stored = read(&file);
    edit(&mut stored);
    write(&file, &stored)
}

fn defaults() -> Stored {
    Stored {
        accent: DEFAULT_ACCENT.to_string(),
        provider: DEFAULT_PROVIDER.to_string(),
        kimi_api_key: String::new(),
        deepseek_api_key: String::new(),
        qwen_api_key: String::new(),
        aws_profile: String::new(),
    }
}

/// What is in `file`, with anything missing or empty filled in from the
/// defaults. A missing file is the first run and a corrupt one is a bug
/// elsewhere; neither is worth refusing to draw the bar over, when the fallback
/// is what it would have had anyway.
fn read(file: &Path) -> Stored {
    let mut stored: Stored = fs::read_to_string(file)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();

    if stored.accent.is_empty() {
        stored.accent = DEFAULT_ACCENT.to_string();
    }
    if stored.provider.is_empty() {
        stored.provider = DEFAULT_PROVIDER.to_string();
    }
    stored
}

fn write(file: &Path, stored: &Stored) -> Result<(), String> {
    let json = serde_json::to_string(stored).map_err(|err| err.to_string())?;
    // The config directory is created lazily by whatever first writes to it, and
    // this is the only thing in the app that ever does.
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    fs::write(file, json).map_err(|err| err.to_string())?;

    // This file holds an API key in cleartext — a decision taken knowingly, for
    // the sake of one storage path. Owner-only is the least that buys.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600))
            .map_err(|err| err.to_string())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path in the temp directory that nothing else in this run will use.
    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("tonemate-{}-{}.json", std::process::id(), name))
    }

    #[test]
    fn missing_file_reads_as_the_defaults() {
        let file = scratch("missing");
        let _ = fs::remove_file(&file);
        let stored = read(&file);
        assert_eq!(stored.accent, DEFAULT_ACCENT);
        assert_eq!(stored.provider, DEFAULT_PROVIDER);
        assert_eq!(stored.kimi_api_key, "");
        assert_eq!(stored.deepseek_api_key, "");
        assert_eq!(stored.qwen_api_key, "");
        assert_eq!(stored.aws_profile, "");
    }

    #[test]
    fn a_written_settings_file_reads_back() {
        let file = scratch("round-trip");
        write(
            &file,
            &Stored {
                accent: "indigo".to_string(),
                provider: "deepseek".to_string(),
                kimi_api_key: "sk-example".to_string(),
                deepseek_api_key: "sk-deepseek".to_string(),
                qwen_api_key: "sk-qwen".to_string(),
                aws_profile: "corp".to_string(),
            },
        )
        .unwrap();
        let stored = read(&file);
        assert_eq!(stored.accent, "indigo");
        assert_eq!(stored.provider, "deepseek");
        assert_eq!(stored.kimi_api_key, "sk-example");
        assert_eq!(stored.deepseek_api_key, "sk-deepseek");
        assert_eq!(stored.qwen_api_key, "sk-qwen");
        assert_eq!(stored.aws_profile, "corp");
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn unreadable_contents_read_as_the_defaults() {
        let file = scratch("corrupt");
        fs::write(&file, "{ not json").unwrap();
        let stored = read(&file);
        assert_eq!(stored.accent, DEFAULT_ACCENT);
        assert_eq!(stored.provider, DEFAULT_PROVIDER);
        let _ = fs::remove_file(&file);
    }

    /// The migration guarantee: a file written before providers existed holds
    /// only an accent, and must keep working rather than resetting the colour.
    #[test]
    fn a_file_from_before_providers_keeps_its_accent() {
        let file = scratch("migrate");
        fs::write(&file, r#"{"accent":"wine"}"#).unwrap();
        let stored = read(&file);
        assert_eq!(stored.accent, "wine");
        assert_eq!(stored.provider, DEFAULT_PROVIDER);
        assert_eq!(stored.kimi_api_key, "");
        assert_eq!(stored.deepseek_api_key, "");
        assert_eq!(stored.qwen_api_key, "");
        let _ = fs::remove_file(&file);
    }

    /// A provider name this build does not know — written by a later version —
    /// is not a reason to refuse to translate.
    #[test]
    fn an_unknown_provider_name_is_kept_verbatim() {
        let file = scratch("unknown-provider");
        fs::write(&file, r#"{"accent":"pine","provider":"openai"}"#).unwrap();
        assert_eq!(read(&file).provider, "openai");
        let _ = fs::remove_file(&file);
    }

    #[test]
    #[cfg(unix)]
    fn the_file_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let file = scratch("permissions");
        write(&file, &Stored::default()).unwrap();
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "settings.json holds an API key");
        let _ = fs::remove_file(&file);
    }
}
