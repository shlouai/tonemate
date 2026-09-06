# Multiple LLM providers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user pick the translation service in the settings window, with Kimi as the first alternative to Bedrock and Bedrock as the default and fallback.

**Architecture:** A new `provider` module owns `SYSTEM_PROMPT` and a `Provider` enum; `provider/bedrock.rs` is today's `bedrock.rs` moved, and `provider/openai_compat.rs` is a transport parameterised by base URL, model and API key so DeepSeek and OpenAI can reuse it later. The SSE decoding is split out as a pure function over byte chunks so it is unit-testable without a network. Settings storage grows from a single accent string to a struct, with `#[serde(default)]` as the migration guarantee.

**Tech Stack:** Rust, Tauri 2, `reqwest` (rustls), `futures-util`, vanilla TypeScript, CSS-only tab switching.

**Spec:** `docs/superpowers/specs/2026-09-05-multiple-providers-design.md`

## Global Constraints

- **Kimi requests MUST send `"thinking": {"type": "disabled"}`.** Without it, reasoning tokens are drawn from the answer budget: measured 21.6s elapsed, 1023 reasoning frames, zero content frames, `finish_reason: length`. This is the single setting that decides whether the provider works.
- **The token cap field is `max_completion_tokens`, not `max_tokens`** (deprecated by Moonshot). Value `4096` for a translation, `1` for a warm-up.
- **Default Kimi base URL is `https://api.moonshot.cn/v1`**; the international host is `https://api.moonshot.ai/v1`. A key valid on one returns `401 Invalid Authentication` on the other.
- **Default Kimi model is `kimi-k2.6`.** `kimi-k2.7-code` forces reasoning on and must not be offered.
- **The API key is never sent to the webview.** Only `{ provider, kimi_key_set: bool }` crosses the IPC boundary.
- **Missing key ⇒ fall back to Bedrock and log it. Failing call ⇒ surface the error, never fall back.**
- **Error strings are prefixed with the provider name**, e.g. `Kimi: Invalid Authentication`.
- `settings.json` is written with `0600` permissions.
- Existing `tones.rs` tests must stay green throughout — they are the regression signal for the refactor.
- Comment style: this codebase explains *why*, not *what*, in full sentences. Match it.

---

### Task 1: Move Bedrock behind a `provider` module

Pure refactor: no behaviour change, no new dependency. Establishes the seam so later tasks add a file rather than restructure.

**Files:**
- Create: `src-tauri/src/provider/mod.rs`
- Create: `src-tauri/src/provider/bedrock.rs` (content moved from `src-tauri/src/bedrock.rs`)
- Delete: `src-tauri/src/bedrock.rs`
- Modify: `src-tauri/src/lib.rs:1` (module list), `src-tauri/src/lib.rs:28` (call site), `src-tauri/src/lib.rs:136-141` (warm-up)
- Modify: `src-tauri/examples/translate.rs:23,38`

**Interfaces:**
- Consumes: nothing (first task)
- Produces:
  - `provider::SYSTEM_PROMPT: &str` (`pub(crate)`)
  - `enum provider::Provider { Bedrock }` — `Kimi(String)` is added in Task 4
  - `provider::Provider::from_settings(app: &tauri::AppHandle) -> Provider`
  - `provider::Provider::from_env() -> Provider`
  - `provider::Provider::label(&self) -> &'static str`
  - `provider::warm(provider: &Provider) -> Result<(), String>` (async)
  - `provider::translate(provider: &Provider, text: &str, on_delta: impl FnMut(&str)) -> Result<(), String>` (async)
  - `provider::env_or(key: &str, fallback: &str) -> String` (`pub(crate)`)
  - `provider::bedrock::converse(system_prompt: &str, text: &str, max_tokens: i32, on_delta: impl FnMut(&str)) -> Result<(String, Option<String>), String>` (async, `pub(super)`)

> **Deviation from the spec, deliberate:** the spec sketched `warm(app: &AppHandle)` / `translate(app, …)`. Taking `&Provider` instead keeps `examples/translate.rs` — which has no `AppHandle` — on the same two functions as the GUI, rather than needing a parallel entry point. `Provider::from_settings` and `Provider::from_env` are the two ways to obtain one.

- [ ] **Step 1: Create the module directory and move the file**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate/src-tauri
mkdir -p src/provider
git mv src/bedrock.rs src/provider/bedrock.rs
```

- [ ] **Step 2: Strip the prompt and the shared helper out of `provider/bedrock.rs`**

In `src/provider/bedrock.rs`, delete the `SYSTEM_PROMPT` constant (the `const SYSTEM_PROMPT: &str = …;` block and its two doc comments, plus the trailing `// \\t above is deliberately…` note) and delete the `env_or` function. Both move to `mod.rs` in Step 3.

Change the module doc comment at the top to:

```rust
//! Translation via Claude on Amazon Bedrock.
//!
//! The call lives in Rust rather than the webview for two reasons: the AWS
//! credential chain (`~/.aws/credentials`) is only reachable from the host
//! process, and the result has to land on the terminal's stdout.
//!
//! The prompt is not this module's business — it arrives as a parameter, so
//! that every provider asks the model for the same thing.
```

Add the import of the moved helper, directly under the existing `use std::collections::HashMap;`:

```rust
use super::env_or;
```

Change `converse` to take the prompt and to be visible to the parent module. Replace its signature and the `.system(...)` line:

```rust
/// One streamed Converse call. Returns everything the model said; `on_delta`
/// sees each text fragment as it lands, so callers can show the translation
/// forming instead of waiting for the full response.
pub(super) async fn converse(
    system_prompt: &str,
    text: &str,
    max_tokens: i32,
    mut on_delta: impl FnMut(&str),
) -> Result<(String, Option<String>), String> {
```

```rust
        .system(SystemContentBlock::Text(system_prompt.to_string()))
```

Delete the `warm` and `translate` functions from this file entirely — they move to `mod.rs`, which is now what decides which provider runs.

- [ ] **Step 3: Write `provider/mod.rs`**

```rust
//! Which service translates, and the one thing every service is asked.
//!
//! The prompt lives here rather than in a transport because it is what decides
//! output quality: two providers holding their own copy would drift, and the
//! drift would look like a model difference.

pub mod bedrock;

use tauri::AppHandle;

/// The direction is the model's call, not a character-class check in Rust: it
/// already reads the text, and input that mixes scripts — a Chinese sentence
/// carrying one English word — would fool any threshold we picked.
///
/// The tab-delimited line format is what lets the answer stream: each label is
/// fixed the moment its tab arrives, so a rendering fills in character by
/// character instead of appearing all at once at the end of the response.
pub(crate) const SYSTEM_PROMPT: &str = "You are a translation engine. If the user's text is English, translate \
     it into natural, idiomatic Chinese; otherwise translate it into natural, idiomatic English.\n\
     Work out what the writer is doing first: what they want from the reader, how they stand in \
     relation to that reader, and how blunt the original was. Then give 3 to 5 renderings that \
     differ in tone, register, and directness, each one the right choice in some concrete \
     situation. If only three are meaningfully different, give three — never pad the list with \
     near-duplicates.\n\
     The first line is the most faithful, most neutral rendering. Each later line sits further \
     from it in tone.\n\
     Output one rendering per line: a label in Chinese of 2 to 4 characters, then a single tab \
     character (ASCII 9, \\t), then the translation. Use only the tab character as the separator—\
     never a fullwidth space, colon, dash, or any other character. No numbering, no blank lines, \
     no markdown, no quotes, no explanation, and never a line break inside a translation.";
// \\t above is deliberately two characters (backslash-t notation): it names the tab for the
// model without putting a real tab in the source, which would be invisible here and in logs.

/// Four or five renderings of the same input, so roughly five times the budget
/// one translation needed.
const MAX_TOKENS: i32 = 4096;

/// The service a translation goes through. An enum rather than a trait: the set
/// is fixed at compile time and picked by a `match`, so the boxing and lifetime
/// work an `async` trait method would need buys nothing.
pub enum Provider {
    Bedrock,
}

impl Provider {
    /// What the user chose, or Bedrock when they chose nothing usable. Read per
    /// call rather than cached, so a change in the settings window takes effect
    /// on the next translation rather than the next launch.
    pub fn from_settings(_app: &AppHandle) -> Self {
        Provider::Bedrock
    }

    /// The provider `examples/translate.rs` should use. It has no `AppHandle`,
    /// so it configures itself from the environment instead of from settings.
    pub fn from_env() -> Self {
        Provider::Bedrock
    }

    /// Names the service in a log line or an error message. With more than one
    /// provider, "API key not valid" does not say whose.
    pub fn label(&self) -> &'static str {
        match self {
            Provider::Bedrock => "bedrock",
        }
    }
}

pub(crate) fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Pay the one-time costs — config load, credential resolution, and the TLS
/// handshake — before the user asks for anything. Measured on a warm AWS
/// profile, that first request carries ~1s the later ones don't, and a
/// one-token budget buys it for a rounding error's worth of tokens. Runs
/// against the configured model, so a bad model id surfaces at startup rather
/// than on the first translation.
pub async fn warm(provider: &Provider) -> Result<(), String> {
    match provider {
        Provider::Bedrock => bedrock::converse(SYSTEM_PROMPT, "hi", 1, |_| {})
            .await
            .map(|_| ()),
    }
}

pub async fn translate(
    provider: &Provider,
    text: &str,
    on_delta: impl FnMut(&str),
) -> Result<(), String> {
    let (translated, stop_reason) = match provider {
        Provider::Bedrock => bedrock::converse(SYSTEM_PROMPT, text, MAX_TOKENS, on_delta).await?,
    };

    if translated.trim().is_empty() {
        return Err(format!(
            "model returned no text (stop reason: {})",
            stop_reason.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    Ok(())
}
```

- [ ] **Step 4: Point `lib.rs` at the new module**

In `src-tauri/src/lib.rs`, replace line 1 (`pub mod bedrock;`) with:

```rust
pub mod provider;
```

In `submit`, replace the `bedrock::translate(&text, …)` call with a provider chosen from settings:

```rust
    let mut parser = tones::Parser::default();
    let chosen = provider::Provider::from_settings(&window.app_handle().clone());
    let result = provider::translate(&chosen, &text, |fragment| {
```

In `setup`, replace the warm-up block:

```rust
            // Off the startup path: the app is usable the moment the hotkey is
            // registered, and by the time anyone types, the provider is reachable.
            let warm_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let chosen = provider::Provider::from_settings(&warm_handle);
                match provider::warm(&chosen).await {
                    Ok(()) => println!("[tonemate] {} warm", chosen.label()),
                    Err(err) => {
                        eprintln!("[tonemate] {} warmup failed: {err}", chosen.label())
                    }
                }
            });
```

- [ ] **Step 5: Point the example at the new module**

In `src-tauri/examples/translate.rs`, replace the two `tonemate_lib::bedrock::…` references. After the `let text = …` block, add the provider, then use it in both calls:

```rust
    tauri::async_runtime::block_on(async {
        let chosen = tonemate_lib::provider::Provider::from_env();
        let warmup = Instant::now();
        if let Err(err) = tonemate_lib::provider::warm(&chosen).await {
            eprintln!("[tonemate] {} warmup failed: {err}", chosen.label());
            std::process::exit(1);
        }
        println!("[tonemate] warm in {:?}", warmup.elapsed());
```

```rust
        let result = tonemate_lib::provider::translate(&chosen, &text, |fragment| {
```

Also update the module doc comment's first line to `//! Exercise the translation path without launching the GUI.`

- [ ] **Step 6: Verify it builds and existing tests still pass**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate/src-tauri
cargo test 2>&1 | tail -20
cargo build --examples 2>&1 | tail -5
cargo clippy --all-targets 2>&1 | grep -E '^(warning|error)' | head -20
```

Expected: all `tones::` and `settings::` tests pass, examples build, clippy clean. This task changes no behaviour, so a failure here is a mistake in the move, not a new bug.

- [ ] **Step 7: Commit**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
git add -A src-tauri/src src-tauri/examples
git commit -m "refactor: move Bedrock behind a provider module

The prompt moves up to provider/mod.rs, so a second provider cannot hold its
own copy and drift on the thing that decides output quality. No behaviour
change."
```

---

### Task 2: Settings storage grows from a string to a struct

**Files:**
- Modify: `src-tauri/src/settings.rs` (whole file)
- Modify: `src-tauri/src/lib.rs:80-84` (command registration)

**Interfaces:**
- Consumes: nothing from Task 1
- Produces:
  - `settings::provider_name(app: &AppHandle) -> String`
  - `settings::kimi_api_key(app: &AppHandle) -> String`
  - `settings::ProviderConfig { provider: String, kimi_key_set: bool }` (`Serialize`)
  - Commands: `settings::provider_config`, `settings::set_provider`, `settings::set_kimi_api_key`
  - Unchanged commands: `settings::accent`, `settings::set_accent`

- [ ] **Step 1: Write the failing tests**

Replace the whole `#[cfg(test)] mod tests` block at the end of `src-tauri/src/settings.rs` with:

```rust
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
    }

    #[test]
    fn a_written_settings_file_reads_back() {
        let file = scratch("round-trip");
        write(
            &file,
            &Stored {
                accent: "indigo".to_string(),
                provider: "kimi".to_string(),
                kimi_api_key: "sk-example".to_string(),
            },
        )
        .unwrap();
        let stored = read(&file);
        assert_eq!(stored.accent, "indigo");
        assert_eq!(stored.provider, "kimi");
        assert_eq!(stored.kimi_api_key, "sk-example");
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
        let _ = fs::remove_file(&file);
    }

    /// A provider name this build does not know — written by a later version —
    /// is not a reason to refuse to translate.
    #[test]
    fn an_unknown_provider_name_is_kept_verbatim() {
        let file = scratch("unknown-provider");
        fs::write(&file, r#"{"accent":"pine","provider":"deepseek"}"#).unwrap();
        assert_eq!(read(&file).provider, "deepseek");
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn the_file_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let file = scratch("permissions");
        write(&file, &Stored::default()).unwrap();
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "settings.json holds an API key");
        let _ = fs::remove_file(&file);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate/src-tauri
cargo test settings 2>&1 | tail -20
```

Expected: compile errors — `Stored` has no `provider` field, `read` returns `String` not `Stored`, `DEFAULT_PROVIDER` undefined.

- [ ] **Step 3: Rewrite the non-test part of `settings.rs`**

Replace everything in `src-tauri/src/settings.rs` above the `#[cfg(test)]` line with:

```rust
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
    /// `"bedrock"` or `"kimi"`. A string rather than an enum so a name written
    /// by a later version survives a round trip through this one instead of
    /// taking the whole file down with it.
    provider: String,
    /// Empty means unset, which is what sends translation back to Bedrock.
    kimi_api_key: String,
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
}

#[tauri::command]
pub fn provider_config(app: AppHandle) -> ProviderConfig {
    let stored = load(&app);
    ProviderConfig {
        provider: stored.provider,
        kimi_key_set: !stored.kimi_api_key.is_empty(),
    }
}

#[tauri::command]
pub fn set_provider(app: AppHandle, provider: String) -> Result<(), String> {
    update(&app, |stored| stored.provider = provider.clone())?;
    println!("[tonemate] provider -> {provider}");
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
```

- [ ] **Step 4: Register the three new commands**

In `src-tauri/src/lib.rs`, replace the `invoke_handler` list:

```rust
        .invoke_handler(tauri::generate_handler![
            submit,
            settings::accent,
            settings::set_accent,
            settings::provider_config,
            settings::set_provider,
            settings::set_kimi_api_key
        ])
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate/src-tauri
cargo test settings 2>&1 | tail -20
```

Expected: 6 passing tests in `settings::tests`.

- [ ] **Step 6: Commit**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
git add src-tauri/src/settings.rs src-tauri/src/lib.rs
git commit -m "feat: store the chosen provider and its API key

serde(default) is the migration guarantee — a settings file written before
providers existed keeps its accent. The file is written 0600, since it now
holds a credential in cleartext."
```

---

### Task 3: An SSE decoder for OpenAI-compatible streams

The one piece with real logic and no network, so it comes with tests first and no `async` anywhere.

**Files:**
- Create: `src-tauri/src/provider/sse.rs`
- Modify: `src-tauri/src/provider/mod.rs` (add `mod sse;`)

**Interfaces:**
- Consumes: nothing
- Produces:
  - `provider::sse::Decoder` with `Default`
  - `Decoder::push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String>`
  - `Decoder::finish_reason(&self) -> Option<&str>`
  - `Decoder::saw_reasoning(&self) -> bool`

> **Deviation from the spec, deliberate:** the spec said the decoder is a pure function over `&str` chunks. It takes `&[u8]` instead. A TCP chunk can split a multi-byte UTF-8 character — and every label this app asks for is Chinese, so that is the common case, not the edge case. Buffering bytes and decoding only complete lines makes the hazard structurally impossible rather than something the transport has to remember.

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/provider/sse.rs` with only the tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Every fragment the decoder emitted across a whole stream.
    fn feed(chunks: &[&str]) -> Result<Vec<String>, String> {
        let mut decoder = Decoder::default();
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(decoder.push(chunk.as_bytes())?);
        }
        Ok(out)
    }

    fn frame(delta: &str) -> String {
        format!("data: {{\"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":null}}]}}\n\n")
    }

    #[test]
    fn content_deltas_become_fragments() {
        let stream = format!(
            "{}{}data: [DONE]\n\n",
            frame(r#"{"content":"直译\t"}"#),
            frame(r#"{"content":"I can't come."}"#)
        );
        assert_eq!(feed(&[&stream]).unwrap(), vec!["直译\t", "I can't come."]);
    }

    /// The opening frame carries a role and an empty string, which is not text.
    #[test]
    fn the_opening_role_frame_emits_nothing() {
        let stream = frame(r#"{"role":"assistant","content":""}"#);
        assert_eq!(feed(&[&stream]).unwrap(), Vec::<String>::new());
    }

    /// Kimi streams reasoning in the same shape as the answer. Letting it
    /// through would put the model's private deliberation in the result box.
    #[test]
    fn reasoning_deltas_are_skipped_but_remembered() {
        let mut decoder = Decoder::default();
        let stream = format!(
            "{}{}",
            frame(r#"{"reasoning_content":"The user wants"}"#),
            frame(r#"{"content":"直译\tHello."}"#)
        );
        assert_eq!(
            decoder.push(stream.as_bytes()).unwrap(),
            vec!["直译\tHello."]
        );
        assert!(decoder.saw_reasoning());
    }

    /// Where this decoder is most likely to break: the transport hands over
    /// whatever the socket gave it, which respects no boundary at all.
    #[test]
    fn a_frame_split_across_chunks_is_reassembled() {
        let whole = format!(
            "{}{}data: [DONE]\n\n",
            frame(r#"{"content":"直译\t"}"#),
            frame(r#"{"content":"晚上好"}"#)
        );
        let bytes = whole.as_bytes();
        // Split at every byte offset, including inside `data:`, inside the JSON,
        // and mid-character in the Chinese.
        for split in 1..bytes.len() {
            let mut decoder = Decoder::default();
            let mut out = Vec::new();
            out.extend(decoder.push(&bytes[..split]).unwrap());
            out.extend(decoder.push(&bytes[split..]).unwrap());
            assert_eq!(
                out.concat(),
                "直译\t晚上好",
                "split at byte {split} changed the result"
            );
        }
    }

    #[test]
    fn several_frames_in_one_chunk_all_arrive() {
        let stream = format!(
            "{}{}{}",
            frame(r#"{"content":"a"}"#),
            frame(r#"{"content":"b"}"#),
            frame(r#"{"content":"c"}"#)
        );
        assert_eq!(feed(&[&stream]).unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn carriage_returns_and_blank_lines_are_tolerated() {
        let stream = "\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        assert_eq!(feed(&[stream]).unwrap(), vec!["a"]);
    }

    /// Named events and comment lines belong to the SSE framing, not to us.
    #[test]
    fn non_data_lines_are_ignored() {
        let stream = format!(": keep-alive\nevent: message\n{}", frame(r#"{"content":"a"}"#));
        assert_eq!(feed(&[&stream]).unwrap(), vec!["a"]);
    }

    /// Forward compatibility: a shape this build does not recognise must not
    /// abort a translation that is otherwise arriving fine.
    #[test]
    fn unrecognised_frames_are_skipped() {
        let stream = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[]}}}}]}}\n\n{}",
            frame(r#"{"content":"a"}"#)
        );
        assert_eq!(feed(&[&stream]).unwrap(), vec!["a"]);
    }

    #[test]
    fn malformed_json_is_skipped() {
        let stream = format!("data: {{ not json\n\n{}", frame(r#"{"content":"a"}"#));
        assert_eq!(feed(&[&stream]).unwrap(), vec!["a"]);
    }

    /// Both error `type`s observed live: the wrong host, and an unpaid account.
    #[test]
    fn an_error_frame_becomes_an_error() {
        for (body, expected) in [
            (
                r#"{"error":{"message":"Invalid Authentication","type":"invalid_authentication_error"}}"#,
                "Invalid Authentication",
            ),
            (
                r#"{"error":{"message":"suspended due to insufficient balance","type":"exceeded_current_quota_error"}}"#,
                "suspended due to insufficient balance",
            ),
        ] {
            let stream = format!("data: {body}\n\n");
            assert_eq!(feed(&[&stream]), Err(expected.to_string()));
        }
    }

    #[test]
    fn the_finish_reason_is_captured() {
        let mut decoder = Decoder::default();
        let stream = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n";
        decoder.push(stream.as_bytes()).unwrap();
        assert_eq!(decoder.finish_reason(), Some("length"));
    }

    /// The failure Kimi produces when reasoning is left on: a full budget of
    /// deliberation and not one word of translation. It has to be reportable as
    /// something other than an empty box.
    #[test]
    fn a_reasoning_only_stream_yields_no_text_but_explains_itself() {
        let mut decoder = Decoder::default();
        let stream = format!(
            "{}data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"length\"}}]}}\n\n",
            frame(r#"{"reasoning_content":"thinking..."}"#)
        );
        assert_eq!(decoder.push(stream.as_bytes()).unwrap(), Vec::<String>::new());
        assert!(decoder.saw_reasoning());
        assert_eq!(decoder.finish_reason(), Some("length"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `mod sse;` to `src-tauri/src/provider/mod.rs`, directly under `pub mod bedrock;`:

```rust
pub mod bedrock;
mod sse;
```

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate/src-tauri
cargo test sse 2>&1 | tail -20
```

Expected: `cannot find type Decoder in this scope`.

- [ ] **Step 3: Write the decoder**

Insert above the `#[cfg(test)]` block in `src-tauri/src/provider/sse.rs`:

```rust
//! Pulls answer text out of an OpenAI-compatible SSE stream.
//!
//! Bytes rather than `&str`, and one line at a time: a chunk off the socket can
//! split a multi-byte character, and every label this app asks the model for is
//! Chinese, so that is the ordinary case rather than the edge case. Buffering
//! bytes and decoding only whole lines makes the split impossible to get wrong.
//!
//! No network and no `async` here, which is the point — this is where the
//! stream format's every quirk is pinned down by a test.

use serde::Deserialize;

/// Only the fields that change what the app does. `serde` ignores the rest,
/// which is what lets a provider add to the shape without breaking us.
#[derive(Deserialize)]
struct Frame {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    /// Undocumented in the streaming schema but present in practice on
    /// reasoning-capable models.
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

#[derive(Default)]
pub struct Decoder {
    /// The line still arriving, up to but not including its newline.
    pending: Vec<u8>,
    /// Why the model stopped, once a frame says so. `length` means the budget
    /// ran out, which is the tell for reasoning left switched on.
    finish_reason: Option<String>,
    /// Whether any reasoning arrived, so an empty answer can say why it is
    /// empty instead of just being blank.
    saw_reasoning: bool,
}

impl Decoder {
    /// The answer fragments this chunk completed. A chunk may contain any number
    /// of whole frames plus a partial one, and all are handled. Frames that
    /// carry no answer text — the opening `role` frame, reasoning, the `[DONE]`
    /// sentinel, anything unrecognised — produce nothing.
    ///
    /// `Err` means the provider reported a failure mid-stream, which ends the
    /// translation: half a set of renderings cannot be told from a whole one.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        self.pending.extend_from_slice(chunk);
        let mut fragments = Vec::new();

        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=newline).collect();
            // Lossy rather than strict: a line that is not valid UTF-8 is a
            // provider bug, and dropping the translation over it would be worse
            // than showing a replacement character.
            let line = String::from_utf8_lossy(&line);
            self.line(line.trim_end_matches(['\n', '\r']), &mut fragments)?;
        }

        Ok(fragments)
    }

    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    pub fn saw_reasoning(&self) -> bool {
        self.saw_reasoning
    }

    /// One complete line. Everything that is not a `data:` payload belongs to
    /// the SSE framing — blank separators, `event:` names, `:` comments — and is
    /// none of this decoder's business.
    fn line(&mut self, line: &str, fragments: &mut Vec<String>) -> Result<(), String> {
        let Some(payload) = line.strip_prefix("data:") else {
            return Ok(());
        };
        let payload = payload.trim();

        // The sentinel is not JSON. The loop ends when the body does, so there
        // is nothing to do but decline to parse it.
        if payload == "[DONE]" {
            return Ok(());
        }

        // A frame this build cannot parse is skipped rather than fatal: a
        // provider adding a shape must not break a translation already arriving.
        let Ok(frame) = serde_json::from_str::<Frame>(payload) else {
            return Ok(());
        };

        if let Some(error) = frame.error {
            return Err(error.message);
        }

        for choice in frame.choices {
            if let Some(reason) = choice.finish_reason {
                self.finish_reason = Some(reason);
            }
            if choice.delta.reasoning_content.is_some() {
                self.saw_reasoning = true;
            }
            // An empty string is what the opening frame carries alongside the
            // role, and it is not a fragment of anything.
            if let Some(content) = choice.delta.content.filter(|text| !text.is_empty()) {
                fragments.push(content);
            }
        }

        Ok(())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate/src-tauri
cargo test sse 2>&1 | tail -25
```

Expected: 12 passing tests in `provider::sse::tests`. `a_frame_split_across_chunks_is_reassembled` exercises every byte offset, so a UTF-8 mistake fails loudly here.

- [ ] **Step 5: Commit**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
git add src-tauri/src/provider/sse.rs src-tauri/src/provider/mod.rs
git commit -m "feat: decode OpenAI-compatible SSE streams

Bytes rather than str, because a socket chunk splits multi-byte characters
and every label this app asks for is Chinese. Reasoning deltas are dropped
but remembered, so a reasoning-only response can explain itself instead of
rendering as an empty box."
```

---

### Task 4: The Kimi transport, wired to the chosen provider

**Files:**
- Create: `src-tauri/src/provider/openai_compat.rs`
- Modify: `src-tauri/src/provider/mod.rs` (add the `Kimi` variant and its arms)
- Modify: `src-tauri/Cargo.toml:20-28` (dependencies)

**Interfaces:**
- Consumes: `provider::sse::Decoder` (Task 3), `settings::provider_name` / `settings::kimi_api_key` (Task 2), `provider::env_or` and `provider::SYSTEM_PROMPT` (Task 1)
- Produces:
  - `provider::openai_compat::Endpoint { base_url: String, model: String, api_key: String, extra: serde_json::Value }`
  - `provider::openai_compat::converse(endpoint: &Endpoint, system_prompt: &str, text: &str, max_tokens: u32, on_delta: impl FnMut(&str)) -> Result<(String, Option<String>), String>` (async, `pub(super)`)
  - `enum provider::Provider { Bedrock, Kimi(String) }`

- [ ] **Step 1: Add the dependencies**

In `src-tauri/Cargo.toml`, add to `[dependencies]` after the `tokio` line:

```toml
# rustls rather than the default native-tls, to keep OpenSSL out of the build.
reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "rustls-tls"] }
futures-util = "0.3"
```

Verify it resolves:

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate/src-tauri
cargo fetch 2>&1 | tail -5
```

- [ ] **Step 2: Write the transport**

Create `src-tauri/src/provider/openai_compat.rs`:

```rust
//! Translation through an OpenAI-compatible chat-completions endpoint.
//!
//! Not "the Kimi client": Kimi, DeepSeek and OpenAI differ only in base URL,
//! model id, and a handful of body fields. Parameterising those three keeps the
//! SSE decoding — the part with all the logic — in one place instead of three.

use std::sync::OnceLock;

use futures_util::StreamExt;
use serde_json::{json, Value};

use super::sse::Decoder;

/// Built on first use so startup stays instant, then reused so later
/// translations skip the TLS handshake. The key travels per request, so one
/// client serves every endpoint and a key change needs no rebuild.
///
/// `OnceLock` rather than the `tokio::sync::OnceCell` Bedrock uses: building a
/// `reqwest::Client` awaits nothing, so there is no reason for the initialiser
/// to be async.
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Where to send the request and what to ask for.
pub struct Endpoint {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// Provider-specific body fields, merged over the common ones. Kimi uses
    /// this to switch reasoning off, which it must.
    pub extra: Value,
}

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(reqwest::Client::new)
}

/// One streamed chat completion. Returns everything the model said and why it
/// stopped; `on_delta` sees each fragment as it lands, so callers can show the
/// translation forming instead of waiting for the whole response.
pub(super) async fn converse(
    endpoint: &Endpoint,
    system_prompt: &str,
    text: &str,
    max_tokens: u32,
    mut on_delta: impl FnMut(&str),
) -> Result<(String, Option<String>), String> {
    let mut body = json!({
        "model": endpoint.model,
        "stream": true,
        // `max_tokens` is deprecated by Moonshot in favour of this.
        "max_completion_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": text },
        ],
    });

    // Merged rather than nested, so a provider can also override a common field
    // if it ever needs to.
    if let (Some(target), Some(extra)) = (body.as_object_mut(), endpoint.extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }

    let response = client()
        .post(format!("{}/chat/completions", endpoint.base_url))
        .bearer_auth(&endpoint.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    // A failure arrives as a normal JSON body, not as a stream. The provider's
    // own message is the useful part — "Invalid Authentication" is what tells a
    // user their key belongs to the other host.
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|json| {
                json.pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("HTTP {status}: {}", body.trim()));
        return Err(message);
    }

    let mut decoder = Decoder::default();
    let mut collected = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| err.to_string())?;
        for fragment in decoder.push(&chunk)? {
            collected.push_str(&fragment);
            on_delta(&fragment);
        }
    }

    // The two ways this endpoint returns 200 and no translation, each named so
    // the bar can show something better than a blank box.
    if collected.trim().is_empty() {
        if decoder.saw_reasoning() {
            return Err(
                "the model spent its whole budget reasoning and returned no translation — \
                 reasoning must be switched off for this model"
                    .to_string(),
            );
        }
        return Err(format!(
            "model returned no text (finish reason: {})",
            decoder.finish_reason().unwrap_or("unknown")
        ));
    }

    Ok((collected, decoder.finish_reason().map(str::to_string)))
}
```

- [ ] **Step 3: Add the `Kimi` variant to `provider/mod.rs`**

Add the module declaration under `mod sse;`:

```rust
pub mod bedrock;
pub mod openai_compat;
mod sse;
```

Add the Kimi defaults under the existing `MAX_TOKENS` constant:

```rust
/// The domestic host. A key issued for the international one
/// (`https://api.moonshot.ai/v1`) returns `401 Invalid Authentication` here and
/// vice versa, and the key string does not say which it is — hence the override.
const KIMI_BASE_URL: &str = "https://api.moonshot.cn/v1";

/// `kimi-k2.6` is the only Kimi model whose reasoning can be switched off;
/// `kimi-k2.7-code` forces it on, which spends the whole budget deliberating
/// and returns no translation.
const KIMI_MODEL: &str = "kimi-k2.6";
```

Replace the `Provider` enum and its `impl` with:

```rust
/// The service a translation goes through. An enum rather than a trait: the set
/// is fixed at compile time and picked by a `match`, so the boxing and lifetime
/// work an `async` trait method would need buys nothing.
pub enum Provider {
    Bedrock,
    Kimi(String),
}

impl Provider {
    /// What the user chose, or Bedrock when they chose nothing usable. Read per
    /// call rather than cached, so a change in the settings window takes effect
    /// on the next translation rather than the next launch.
    ///
    /// A provider selected without a key falls back rather than failing: an
    /// empty key means "not configured yet", which is not the same as a key the
    /// service rejected.
    pub fn from_settings(app: &AppHandle) -> Self {
        match crate::settings::provider_name(app).as_str() {
            "kimi" => match crate::settings::kimi_api_key(app) {
                key if key.is_empty() => {
                    println!("[tonemate] kimi selected but no api key set; using bedrock");
                    Provider::Bedrock
                }
                key => Provider::Kimi(key),
            },
            _ => Provider::Bedrock,
        }
    }

    /// The provider `examples/translate.rs` should use. It has no `AppHandle`,
    /// so it configures itself from the environment instead of from settings.
    pub fn from_env() -> Self {
        match std::env::var("TONEMATE_KIMI_API_KEY") {
            Ok(key) if !key.is_empty() => Provider::Kimi(key),
            _ => Provider::Bedrock,
        }
    }

    /// Names the service in a log line or an error message. With more than one
    /// provider, "API key not valid" does not say whose.
    pub fn label(&self) -> &'static str {
        match self {
            Provider::Bedrock => "bedrock",
            Provider::Kimi(_) => "kimi",
        }
    }

    /// Where a Kimi request goes and what it asks for.
    ///
    /// `thinking: disabled` is not optional. Left on, reasoning tokens come out
    /// of the same budget as the answer: measured at 21.6s for 1023 reasoning
    /// frames, zero content frames and `finish_reason: length` — twenty-one
    /// seconds for no translation at all.
    fn kimi_endpoint(key: &str) -> openai_compat::Endpoint {
        openai_compat::Endpoint {
            base_url: env_or("TONEMATE_KIMI_BASE_URL", KIMI_BASE_URL),
            model: env_or("TONEMATE_KIMI_MODEL", KIMI_MODEL),
            api_key: key.to_string(),
            extra: serde_json::json!({ "thinking": { "type": "disabled" } }),
        }
    }
}
```

Replace `warm` and `translate` with versions that carry both arms. Note that the empty-answer check moves into each transport, since each has its own vocabulary for why a stream carried no text:

```rust
pub async fn warm(provider: &Provider) -> Result<(), String> {
    let result = match provider {
        Provider::Bedrock => bedrock::converse(SYSTEM_PROMPT, "hi", 1, |_| {})
            .await
            .map(|_| ()),
        // A one-token budget makes the answer empty by construction, which the
        // transport would otherwise call a failure. Only reachability is being
        // tested here, so that error is the success case.
        Provider::Kimi(key) => {
            match openai_compat::converse(&Provider::kimi_endpoint(key), SYSTEM_PROMPT, "hi", 1, |_| {})
                .await
            {
                Ok(_) => Ok(()),
                Err(err) if err.starts_with("model returned no text") => Ok(()),
                Err(err) => Err(err),
            }
        }
    };

    result.map_err(|err| format!("{}: {err}", provider.label()))
}

pub async fn translate(
    provider: &Provider,
    text: &str,
    on_delta: impl FnMut(&str),
) -> Result<(), String> {
    let result = match provider {
        Provider::Bedrock => bedrock::converse(SYSTEM_PROMPT, text, MAX_TOKENS, on_delta).await,
        Provider::Kimi(key) => {
            openai_compat::converse(
                &Provider::kimi_endpoint(key),
                SYSTEM_PROMPT,
                text,
                MAX_TOKENS as u32,
                on_delta,
            )
            .await
        }
    };

    // Prefixed here rather than in each transport, so every provider's failures
    // read the same way in the bar and in the log.
    result.map(|_| ()).map_err(|err| format!("{}: {err}", provider.label()))
}
```

Then move Bedrock's own empty-answer check into `provider/bedrock.rs`, at the end of `converse`, just before `Ok((collected, stop_reason))`:

```rust
    if collected.trim().is_empty() {
        return Err(format!(
            "model returned no text (stop reason: {})",
            stop_reason.unwrap_or_else(|| "unknown".to_string())
        ));
    }

```

Because `warm` asks for one token, that error is expected there — `warm`'s Bedrock arm needs the same tolerance the Kimi arm has:

```rust
        Provider::Bedrock => match bedrock::converse(SYSTEM_PROMPT, "hi", 1, |_| {}).await {
            Ok(_) => Ok(()),
            Err(err) if err.starts_with("model returned no text") => Ok(()),
            Err(err) => Err(err),
        },
```

- [ ] **Step 4: Verify it compiles and nothing regressed**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate/src-tauri
cargo test 2>&1 | tail -25
cargo clippy --all-targets 2>&1 | grep -E '^(warning|error)' | head -20
```

Expected: all `tones`, `settings` and `sse` tests pass; clippy clean.

- [ ] **Step 5: Verify against the live Kimi API**

Replace `<KEY>` with a working Moonshot key. Do not paste the key into any file.

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate/src-tauri
TONEMATE_KIMI_API_KEY='<KEY>' cargo run --example translate -- "我明天不能来了"
```

Expected: `[tonemate] warm in …`, then tab-delimited rows streaming, then a
`[tonemate] parsed 3-5 tones:` block with non-empty labels and translations, and
`first word in ~2.5s, total ~3.5s`.

Then confirm the two failure paths read well:

```bash
# A key for the other host: expect `kimi: Invalid Authentication`, not a blank result.
TONEMATE_KIMI_API_KEY='<KEY>' TONEMATE_KIMI_BASE_URL='https://api.moonshot.ai/v1' \
  cargo run --example translate -- "test"

# Reasoning left on: expect the "spent its whole budget reasoning" error.
TONEMATE_KIMI_API_KEY='<KEY>' TONEMATE_KIMI_MODEL='kimi-k2.7-code' \
  cargo run --example translate -- "test"
```

If the second command instead returns a translation, `kimi-k2.7-code` behaves
differently than measured — record what happened in the spec rather than
loosening the error.

- [ ] **Step 6: Commit**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/provider
git commit -m "feat: translate through Kimi over an OpenAI-compatible endpoint

Parameterised by base URL, model and key rather than written as a Kimi
client, so DeepSeek and OpenAI are later a constant each.

Selecting a provider with no key stored falls back to Bedrock; a key the
service rejects is surfaced instead, since those two mean different things."
```

---

### Task 5: Two settings panels, and the translation-service panel

**Files:**
- Modify: `settings.html:13-66` (the whole `<main>`)
- Modify: `src/settings.css` (add tab and service-panel rules)
- Modify: `src/settings.ts` (whole file)
- Modify: `src-tauri/tauri.conf.json:36` (window height)

**Interfaces:**
- Consumes: commands `provider_config`, `set_provider`, `set_kimi_api_key` (Task 2)
- Produces: nothing consumed by later tasks

- [ ] **Step 1: Grow the settings window**

The service panel does not fit in 320px, and the window is `resizable: false`. In `src-tauri/tauri.conf.json`, change the settings window's height:

```json
        "height": 400,
```

- [ ] **Step 2: Restructure `settings.html`**

Replace the entire `<main>` element in `settings.html` with:

```html
    <main>
      <h1>设置</h1>

      <!-- The tabs are a radio group so ← → move between them and the panel
           switch needs no JavaScript: the inputs precede both the tab bar and
           the panels, so `:checked ~` can reach them. Provider settings are
           kept off the colour panel deliberately — there will be more services
           than colours before long. -->
      <div class="tabbed">
        <input type="radio" name="tab" id="tab-appearance" checked />
        <input type="radio" name="tab" id="tab-service" />

        <div class="tabbar">
          <label for="tab-appearance">外观</label>
          <label for="tab-service">翻译服务</label>
        </div>

        <section class="panel" id="panel-appearance">
          <!-- A radio group, so ← → move between the colours and the choice
               reads as one setting with six values rather than six switches.
               The swatch is drawn by the label: the input itself is taken off
               screen rather than hidden, because `display: none` would also
               take it out of the tab order. -->
          <fieldset id="accent">
            <legend>输入条颜色</legend>
            <!-- The swatches are laid out in a wrapper rather than on the
                 fieldset itself: a legend is not an ordinary child, and engines
                 disagree about what it does inside a grid. -->
            <div class="swatches">
              <label>
                <input type="radio" name="accent" value="graphite" />
                <span class="swatch" data-accent="graphite"></span>
                石墨
              </label>
              <label>
                <input type="radio" name="accent" value="indigo" />
                <span class="swatch" data-accent="indigo"></span>
                靛蓝
              </label>
              <label>
                <input type="radio" name="accent" value="pine" />
                <span class="swatch" data-accent="pine"></span>
                墨绿
              </label>
              <label>
                <input type="radio" name="accent" value="wine" />
                <span class="swatch" data-accent="wine"></span>
                酒红
              </label>
              <label>
                <input type="radio" name="accent" value="violet" />
                <span class="swatch" data-accent="violet"></span>
                紫罗兰
              </label>
              <label>
                <input type="radio" name="accent" value="amber" />
                <span class="swatch" data-accent="amber"></span>
                琥珀
              </label>
            </div>
          </fieldset>
        </section>

        <section class="panel" id="panel-service">
          <fieldset id="provider">
            <legend>翻译服务</legend>
            <div class="providers">
              <label>
                <input type="radio" name="provider" value="bedrock" />
                <span class="name">AWS Bedrock</span>
                <span class="note">环境中的 AWS profile</span>
              </label>
              <label>
                <input type="radio" name="provider" value="kimi" />
                <span class="name">Kimi</span>
                <!-- Filled in from `kimi_key_set`; the key itself never reaches
                     this window. -->
                <span class="note" id="kimi-state"></span>
              </label>
            </div>
          </fieldset>

          <div class="key">
            <label for="kimi-key">Kimi API Key</label>
            <div class="key-row">
              <input
                type="password"
                id="kimi-key"
                autocomplete="off"
                spellcheck="false"
              />
              <button type="button" id="kimi-clear">清除</button>
            </div>
            <p class="hint" id="kimi-hint"></p>
          </div>

          <p id="active"></p>
        </section>
      </div>
    </main>
```

- [ ] **Step 3: Add the CSS**

Append to `src/settings.css`:

```css
/* The inputs come first in the markup so `:checked ~` can reach both the tab
   bar and the panels. Off screen rather than `display: none`, for the same
   reason the accent radios are: the tab strip has to stay keyboard-reachable. */
.tabbed > input {
  position: absolute;
  opacity: 0;
  pointer-events: none;
}

.tabbar {
  display: flex;
  gap: 4px;
  margin-bottom: 16px;
  border-bottom: 1px solid color-mix(in srgb, CanvasText 15%, transparent);
}

.tabbar label {
  padding: 6px 12px;
  font-size: 13px;
  cursor: pointer;
  /* The active tab is marked with a rule flush against the strip's own border,
     so the two read as one edge rather than as two lines. */
  border-bottom: 2px solid transparent;
  margin-bottom: -1px;
  opacity: 0.6;
  -webkit-user-select: none;
  user-select: none;
}

.tabbar label:hover {
  opacity: 0.85;
}

.panel {
  display: none;
}

#tab-appearance:checked ~ .tabbar label[for="tab-appearance"],
#tab-service:checked ~ .tabbar label[for="tab-service"] {
  opacity: 1;
  font-weight: 600;
  border-bottom-color: CanvasText;
}

#tab-appearance:checked ~ #panel-appearance,
#tab-service:checked ~ #panel-service {
  display: block;
}

/* Focus has to be shown on the label, since the input it belongs to cannot be
   seen. Addressed by id rather than by position: a positional selector would
   silently target the wrong tab the moment a third one is added. */
#tab-appearance:focus-visible ~ .tabbar label[for="tab-appearance"],
#tab-service:focus-visible ~ .tabbar label[for="tab-service"] {
  outline: 2px solid CanvasText;
  outline-offset: 2px;
}

/* Same reasoning as #accent: the fieldset is for the grouping and the legend,
   not for a frame. */
#provider {
  margin: 0;
  padding: 0;
  border: none;
}

#provider legend {
  padding: 0;
  font-size: 13px;
  font-weight: 600;
}

/* One per row rather than side by side: each carries a note, and the list grows
   as services are added. */
.providers {
  margin-top: 10px;
  display: grid;
  gap: 4px;
}

#provider label {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 7px 8px;
  border-radius: 8px;
  font-size: 13px;
  cursor: pointer;
  -webkit-user-select: none;
  user-select: none;
}

#provider label:hover {
  background: color-mix(in srgb, CanvasText 8%, transparent);
}

/* Visible here, unlike the accent radios: there is no swatch standing in for
   the control, so the dot is what says which service is chosen. */
#provider input {
  margin: 0;
}

#provider .note {
  font-size: 12px;
  opacity: 0.55;
}

.key {
  margin-top: 18px;
}

.key > label {
  display: block;
  font-size: 13px;
  font-weight: 600;
}

.key-row {
  margin-top: 8px;
  display: flex;
  gap: 8px;
}

#kimi-key {
  flex: 1 1 auto;
  min-width: 0;
  box-sizing: border-box;
  padding: 5px 8px;
  font: inherit;
  font-size: 13px;
}

#kimi-clear {
  flex: 0 0 auto;
  font: inherit;
  font-size: 13px;
}

.hint {
  /* Overrides the `p` rule's 20px top margin: this sits directly under the
     field it describes. */
  margin: 6px 0 0;
  font-size: 12px;
}
```

- [ ] **Step 4: Rewrite `src/settings.ts`**

```typescript
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

const appWindow = getCurrentWindow();

/** What Rust will say about the translation service. Never the key itself. */
type ProviderConfig = { provider: string; kimi_key_set: boolean };

window.addEventListener("DOMContentLoaded", () => {
  wireAccent();
  void wireProvider();
});

// Which swatch is ticked comes from Rust, which is where the accent is kept:
// asking is the only way to open on the colour the bar is actually wearing, from
// this launch or any earlier one. Read once, because this window is the only
// thing that ever writes it. An accent no swatch carries ticks none of them,
// which is the honest answer — the bar is showing something this window can no
// longer offer.
function wireAccent() {
  const radios = document.querySelectorAll<HTMLInputElement>(
    '#accent input[name="accent"]',
  );

  void invoke<string>("accent").then((accent) => {
    for (const radio of radios) radio.checked = radio.value === accent;
  }, console.error);

  // On `change` rather than on click, so choosing with ← → counts too. Fire and
  // forget: Rust tells the bar to repaint, and there is nothing to show here
  // beyond the swatch the browser has already ticked.
  for (const radio of radios) {
    radio.addEventListener("change", () => {
      if (radio.checked)
        void invoke("set_accent", { accent: radio.value }).catch(console.error);
    });
  }
}

async function wireProvider() {
  const radios = document.querySelectorAll<HTMLInputElement>(
    '#provider input[name="provider"]',
  );
  const keyInput = document.querySelector<HTMLInputElement>("#kimi-key");
  const clearButton = document.querySelector<HTMLButtonElement>("#kimi-clear");
  if (!keyInput || !clearButton) return;

  let config = await invoke<ProviderConfig>("provider_config").catch(
    (error): ProviderConfig => {
      console.error(error);
      // Bedrock needs nothing configured, so it is the safe thing to show when
      // the question could not be asked.
      return { provider: "bedrock", kimi_key_set: false };
    },
  );

  render();

  for (const radio of radios) {
    radio.addEventListener("change", () => {
      if (!radio.checked) return;
      config = { ...config, provider: radio.value };
      render();
      void invoke("set_provider", { provider: radio.value }).catch(
        console.error,
      );
    });
  }

  // On `change`, so the key is sent on blur or Enter rather than on every
  // keystroke — a key is pasted, not typed, and each save writes the file and
  // spends a warm-up request.
  //
  // An empty field means "unchanged", not "clear": the stored key is never sent
  // to this window, so an empty box is what a configured key looks like here.
  // Clearing is the button's job alone.
  keyInput.addEventListener("change", () => {
    const key = keyInput.value.trim();
    if (!key) return;
    keyInput.value = "";
    config = { ...config, kimi_key_set: true };
    render();
    void invoke("set_kimi_api_key", { key }).catch(console.error);
  });

  clearButton.addEventListener("click", () => {
    keyInput.value = "";
    config = { ...config, kimi_key_set: false };
    render();
    void invoke("set_kimi_api_key", { key: "" }).catch(console.error);
  });

  function render() {
    for (const radio of radios) radio.checked = radio.value === config.provider;

    const state = document.querySelector<HTMLElement>("#kimi-state");
    if (state) state.textContent = config.kimi_key_set ? "已配置" : "未配置";

    const hint = document.querySelector<HTMLElement>("#kimi-hint");
    if (hint)
      hint.textContent = config.kimi_key_set
        ? "已保存。输入新的 key 可替换,或点「清除」删除。"
        : "在 platform.moonshot.cn 获取。国际站的 key 需要设置 TONEMATE_KIMI_BASE_URL。";

    // Says what will actually happen on the next translation, which is not
    // always what is ticked: Kimi without a key falls back to Bedrock, and
    // saying so here is cheaper than letting the log be the only place it shows.
    const active = document.querySelector<HTMLElement>("#active");
    if (active)
      active.textContent =
        config.provider === "kimi" && !config.kimi_key_set
          ? "当前使用: AWS Bedrock —— 已选 Kimi 但未填 API Key"
          : `当前使用: ${config.provider === "kimi" ? "Kimi" : "AWS Bedrock"}`;
  }
}

// Esc closes the window, which for this one means hiding it — Rust turns every
// close request into a hide. Asking to close rather than hiding directly is
// deliberate: it makes Esc, ⌘W and the red button one path instead of three, so
// there is a single place where what "closing the settings" means can change.
// Bound to the window rather than to a control: there is nothing here to focus
// yet, so a handler on an element would never fire.
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  // Esc in a half-typed key field abandons what was typed rather than dismissing
  // the window — the same thing Esc does in every other text field on this
  // system, and the difference between losing a keystroke and losing the window.
  // Only the password field: a focused radio is also an HTMLInputElement, and
  // its `value` is the accent or provider name rather than something typed.
  const focused = document.activeElement;
  if (
    focused instanceof HTMLInputElement &&
    focused.type === "password" &&
    focused.value !== ""
  ) {
    focused.value = "";
    e.preventDefault();
    return;
  }
  e.preventDefault();
  void appWindow.close().catch(console.error);
});
```

- [ ] **Step 5: Type-check and build**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
pnpm build 2>&1 | tail -15
```

Expected: no TypeScript errors; `dist/settings.html` and hashed assets written.

- [ ] **Step 6: Render both panels headlessly and look at them**

The GUI cannot be driven from this environment, so the layout is checked as a
static page. Serve `dist/` so the hashed asset paths resolve:

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
python3 -m http.server 8765 --directory dist &
sleep 1
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
"$CHROME" --headless --disable-gpu --window-size=480,400 \
  --screenshot=/tmp/tonemate-settings-appearance.png \
  "http://localhost:8765/settings.html"
"$CHROME" --headless --disable-gpu --window-size=480,400 \
  --screenshot=/tmp/tonemate-settings-service.png \
  "http://localhost:8765/settings.html#" \
  --virtual-time-budget=1500
kill %1
```

Then read both PNGs. The second needs the service tab selected; since the tab is
CSS-only there is no URL for it, so verify that panel by temporarily moving
`checked` from `#tab-appearance` to `#tab-service` in `settings.html`, rebuilding,
re-shooting, and moving it back.

Check: the tab strip reads as two tabs with the active one marked; the six
swatches are unchanged from before this task; the service panel shows two
providers, the key field with its 清除 button, and both hint and active lines
without overflowing 400px.

- [ ] **Step 7: Launch the app and confirm the panel works end to end**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
pnpm tauri dev
```

By hand: open 设置 from the menu bar, switch to 翻译服务, select Kimi with no key
(expect the log line `kimi selected but no api key set; using bedrock` on the next
translation and `当前使用: AWS Bedrock …` in the window), paste a key (expect
`[tonemate] kimi api key -> set` and `[tonemate] kimi warm`), then translate with
`Cmd+Shift+Space` and confirm renderings stream. Confirm the terminal never
prints the key itself. Then 清除 and confirm it goes back to Bedrock.

- [ ] **Step 8: Commit**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
git add settings.html src/settings.css src/settings.ts src-tauri/tauri.conf.json
git commit -m "feat: pick the translation service in settings

Two panels rather than one list: there will be more services than colours
before long, and mixing them would make the colour setting hard to find.
Switching is CSS-only, the same radio-group mechanism the swatches use.

The window shows what will actually happen next — Kimi without a key says
Bedrock — rather than only what is ticked."
```

---

### Task 6: Update the README

**Files:**
- Modify: `README.md:1-8` (intro), `README.md:10-19` (Prerequisites), `README.md:37-41` (menu bar paragraph), `README.md:68-78` (CLI section), `README.md:80-107` (Configuration)

**Interfaces:**
- Consumes: everything above
- Produces: nothing

- [ ] **Step 1: Rewrite the intro's last sentence**

Replace `Translation runs through Claude on Amazon Bedrock.` with:

```markdown
Translation runs through Claude on Amazon Bedrock by default, or through Kimi if
you paste an API key into the settings window.
```

- [ ] **Step 2: Rewrite Prerequisites**

Replace the Prerequisites section with:

```markdown
## Prerequisites

- [Rust toolchain](https://rustup.rs) and [pnpm](https://pnpm.io)
- Credentials for one of the two translation services:
  - **AWS Bedrock** (the default) — credentials that can call Bedrock in the
    configured region.

  - **Kimi** — an API key from [platform.moonshot.cn](https://platform.moonshot.cn),
    pasted into the settings window. Nothing else to install.
```

- [ ] **Step 3: Rewrite the menu-bar paragraph's last sentence**

Replace `The settings window holds the bar's colour; everything under [Configuration](#configuration) is still set through the environment.` with:

```markdown
The settings window has two panels: 外观 holds the bar's colour, and 翻译服务
picks the translation service and holds its API key. Model ids and hosts are
still set through the environment — see [Configuration](#configuration).
```

- [ ] **Step 4: Rewrite the CLI section**

Replace the `## Translating without the GUI` section body with:

```markdown
To exercise the translation path directly — useful for checking credentials or
timing a model change:

```sh
cd src-tauri
cargo run --example translate -- "今天天气不错，我们出去走走吧。"
```

It prints the raw stream, then a `[tonemate] parsed N tones:` block listing each
labelled rendering, then time-to-first-word and total time. It uses Bedrock
unless `TONEMATE_KIMI_API_KEY` is set, since it has no access to the settings
window's choice:

```sh
TONEMATE_KIMI_API_KEY=sk-… cargo run --example translate -- "我明天不能来了"
```
```

- [ ] **Step 5: Rewrite Configuration**

Replace everything from `## Configuration` up to (not including) `## Building a release binary` with:

```markdown
## Configuration

The settings window holds the two choices that belong to a person rather than to
a machine, and both take effect immediately:

- **输入条颜色** — 石墨, 靛蓝, 墨绿, 酒红, 紫罗兰 or 琥珀, and the bar repaints as
  you choose. All six are dark panes of the same lightness, because the text,
  borders and grip drawn on them are white at some opacity; a light bar would be
  a second theme rather than a colour.
- **翻译服务** — AWS Bedrock or Kimi. Choosing Kimi without saving a key falls
  back to Bedrock and says so, since an empty key means "not set up yet". A key
  that the service *rejects* is reported instead of falling back — otherwise a
  revoked key would silently bill AWS forever.

Both live in
`~/Library/Application Support/com.lous008.tonemate/settings.json`, so they
survive a restart. **The Kimi API key is stored there in cleartext**, readable by
anything running as you; the file is written owner-only, which is a speed bump
rather than protection.

Everything else is environment-only. All optional; each overrides the default
shown.

| Variable | Default | Applies to |
| --- | --- | --- |
| `TONEMATE_AWS_PROFILE` | — | Bedrock |
| `TONEMATE_AWS_REGION` | `us-west-2` | Bedrock |
| `TONEMATE_MODEL` | `us.anthropic.claude-opus-5` | Bedrock |
| `TONEMATE_EFFORT` | `low` | Bedrock |
| `TONEMATE_KIMI_BASE_URL` | `https://api.moonshot.cn/v1` | Kimi |
| `TONEMATE_KIMI_MODEL` | `kimi-k2.6` | Kimi |
| `TONEMATE_KIMI_API_KEY` | — | the CLI example only |

Bedrock serves the Claude 5 family only through cross-region inference profiles,
so `TONEMATE_MODEL` needs the `us.` prefix — a bare `anthropic.claude-opus-5` is
rejected. `TONEMATE_EFFORT` set to the empty string drops the parameter from the
request entirely, which is what lets `TONEMATE_MODEL` point at Haiku 4.5 or other
models that reject it:

```sh
TONEMATE_MODEL=us.anthropic.claude-haiku-4-5-20251001-v1:0 TONEMATE_EFFORT= pnpm tauri dev
```

Two Kimi notes worth knowing before changing either variable. Moonshot runs a
domestic host (`api.moonshot.cn`) and an international one
(`api.moonshot.ai`), and a key issued for one returns `401 Invalid
Authentication` on the other — the key string does not say which it is, so a
`401` usually means the wrong `TONEMATE_KIMI_BASE_URL`. And `TONEMATE_KIMI_MODEL`
should stay on `kimi-k2.6`: it is the only Kimi model whose reasoning can be
switched off, and with reasoning on, the model spends the entire token budget
deliberating and returns no translation at all.
```

- [ ] **Step 6: Check the rendered result**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
grep -n 'TONEMATE_' README.md
```

Expected: seven variables, matching the table above and the constants in
`src-tauri/src/provider/mod.rs` and `src-tauri/src/provider/bedrock.rs`. Any
name here that no longer exists in the code is a documentation bug.

- [ ] **Step 7: Commit**

```bash
cd /Users/LOUS008/Workspace/Dev/tomorrow/tonemate
git add README.md
git commit -m "docs: describe both translation services

Says plainly that the Kimi key is stored in cleartext, and why a 401 usually
means the wrong host rather than a bad key."
```

---

## Self-Review Notes

Checked against the spec:

- **Enum not trait** — Task 1 Step 3, Task 4 Step 3.
- **Transport parameterised by base URL** — Task 4 Step 2 (`Endpoint`).
- **Explicit selection, Bedrock fallback** — Task 4 Step 3 (`from_settings`), Task 5 Step 4 (the 当前使用 line).
- **Missing key falls back / failing call surfaces** — Task 4 Step 3, verified in Task 5 Step 7.
- **Provider stored as a string, unknown degrades to Bedrock** — Task 2 Step 1 (`an_unknown_provider_name_is_kept_verbatim`), Task 4 Step 3 (`_ => Provider::Bedrock`).
- **Cleartext key, 0600** — Task 2 Step 3, tested in `the_file_is_written_owner_only`, documented in Task 6 Step 5.
- **Key never sent to the webview** — `ProviderConfig` carries a bool (Task 2 Step 3); Task 5 Step 4 never reads a key back.
- **Empty field means unchanged, 清除 clears** — Task 5 Step 4.
- **Two panels, CSS-only switching** — Task 5 Steps 2-3.
- **`thinking: disabled`** — Task 4 Step 3 (`kimi_endpoint`), asserted live in Task 4 Step 5.
- **`max_completion_tokens`** — Task 4 Step 2.
- **Latency parity** — Task 4 Step 5 expectations.
- **`reasoning_content` skipped** — Task 3 (`reasoning_deltas_are_skipped_but_remembered`).
- **Three distinguishable "200 but no translation" cases** — Task 4 Step 2 (reasoning-only, and finish-reason-with-no-content; whitespace-only falls into the latter).
- **Two hosts** — Task 4 Step 3 (`KIMI_BASE_URL`), Task 4 Step 5 (wrong-host check), Task 6 Step 5.
- **Env overrides** — Task 4 Step 3, Task 6 Step 5.
- **`reqwest` rustls + `futures-util`** — Task 4 Step 1.
- **Decoder tests** — Task 3 Step 1 covers every case the spec's Testing section lists.
- **Settings migration test** — Task 2 Step 1 (`a_file_from_before_providers_keeps_its_accent`).
- **`tones.rs` untouched and green** — verified in Task 1 Step 6 and Task 4 Step 4.

Two spec items intentionally not implemented, and why:

- **Gemini** — the spec's `Deferred: Gemini` section is a record, not a requirement. No task implements it.
- **The spec's `warm(app)` / `translate(app, …)` signatures and `&str` SSE chunks** — both deviated from, each flagged inline with its reason (the example has no `AppHandle`; a socket chunk splits multi-byte characters).

One spec sentence this plan tightened: the spec said the empty-answer check lives
in `provider/mod.rs`. It moves into each transport instead, because Bedrock's
vocabulary for it is `stop_reason` and Kimi's is `finish_reason` plus "was there
reasoning" — one shared check could only report the vaguer of the two. The
consequence is that `warm`, which asks for one token and so is empty by
construction, has to tolerate that specific error in both arms; that is written
out in Task 4 Step 3.
