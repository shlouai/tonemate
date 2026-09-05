# Multiple LLM providers

## Problem

Translation is wired directly to Claude on Amazon Bedrock. That binds the app to
one credential story: an AWS profile in `~/.aws/config`, refreshed by `aws sso
login` when it expires. Anyone without that profile cannot run tonemate at all,
and there is no way to trade the SSO dance for an API key pasted into a text
field.

The seam is also missing rather than badly placed. `bedrock.rs` owns both *how to
reach a model* and *what to ask it* — `SYSTEM_PROMPT`, the thing that actually
determines output quality, lives inside the transport. A second provider added
beside it would either duplicate that prompt or reach into another module for it.

## Goal

Let the user pick the translation service in the settings window. Gemini is the
first alternative: paste an API key, and translations go through Google instead
of Bedrock. Bedrock remains the default and the fallback, so an install with no
API key configured behaves exactly as it does today.

Structure it so that DeepSeek and OpenAI are each one new file and one new radio
button, not a re-litigation of this design.

## Out of scope

- DeepSeek, OpenAI, or any third provider (this change only makes room for them)
- Per-provider model pickers in the UI (model stays env-overridable)
- Showing a "key is valid" verdict in the settings window; a bad key surfaces in
  the log at save time and as an error on the first translation
- Storing the key anywhere but `settings.json` (see Decisions)
- Streaming tool calls, multi-turn history, or anything else the Interactions API
  offers beyond one-shot streamed text

## Decisions

**An enum, not a trait.** `Provider` is an enum with one variant per service and
a two-arm `match` in `provider/mod.rs`. A trait with `async fn` returning a
stream needs boxing and a lifetime dance around the `on_delta` closure, and buys
runtime polymorphism that nothing here wants: the set of providers is fixed at
compile time and selected by an `if`. Leaving `bedrock.rs` in place and branching
in `lib.rs` was the third option, rejected because the branch would then be
duplicated in `examples/translate.rs` and `SYSTEM_PROMPT` would need two homes.

**The provider is chosen explicitly, with Bedrock as the fallback.** A radio
group in the settings window, persisted as `provider` in `settings.json`. The
rejected alternative — infer the provider from which API key is filled in — has
no answer once three services each have a key, and makes "switch back to
Bedrock" mean "delete your key". Keys are stored per provider and survive
switching.

Two failure modes that must not be conflated:

| Situation | Behaviour |
| --- | --- |
| Selected provider has no key stored | Fall back to Bedrock, log which and why |
| Selected provider has a key, but the call fails | Surface the error, do **not** fall back |

A missing key means "not configured yet". A rejected key means something is
wrong that silently billing AWS would hide. This is why the error string is
prefixed with the provider name: with two sources, `API key not valid` alone does
not say whose.

**`provider` is stored as a string, not a serialised enum.** An unknown provider
name — written by a future version, read by this one — degrades to Bedrock. Same
tolerance `accent` already has for a colour `palette.css` no longer defines.

**The key is stored in cleartext in `settings.json`,** next to `accent`, at
`~/Library/Application Support/com.lous008.tonemate/settings.json`. Chosen over
the macOS Keychain for the sake of reusing the existing read/write path and
avoiding a Keychain prompt on first save. Any process running as the user can
read it. The file is written `0600`, which is a speed bump and not protection.
This is a deliberate, recorded trade-off, not an oversight.

**The key is never sent to the webview.** Opening settings yields
`{ provider, gemini_key_set: bool }`; the input renders empty with a placeholder
saying a key is already stored. The user cannot read the stored key back from the
UI — `cat` the JSON for that. Consequence: an empty field on save is ambiguous
between "unchanged" and "clear it", so clearing needs an explicit button.

**Two settings panels, one window.** Appearance (the colour swatches) and
translation service become separate panels in the existing `settings.html`, not
separate windows — the settings window is already reused and hidden on close, and
a second window would mean a second lifecycle to manage. Panels switch via a
radio group plus CSS sibling selectors, the same mechanism the accent swatches
already use, so no JavaScript is involved in switching and arrow keys move
between tabs.

**Gemini is reached through the Interactions API, not `generateContent`.** This
was forced by probing the live API, against both the documentation and the
author's assumptions:

- `models/gemini-2.5-flash:streamGenerateContent` returns HTTP 404 — *"This model
  models/gemini-2.5-flash is no longer available to new users. Please update your
  code to use models/gemini-3.6-flash for the latest features and improvements.
  We recommend you to use the Interactions API."*
- `ListModels` for a real key returns 54 models, of which **zero** advertise
  `streamGenerateContent`. Every text model advertises `generateContent`.
- `ai.google.dev` still documents `gemini-2.5-flash` as an active model. The API
  disagrees. The API wins.

So the legacy streaming endpoint is treated as closed to new projects, and the
default model becomes `gemini-3.8-flash` — the current Flash tier, matching the
"fast and cheap over maximum quality" call, since translation with a fixed system
prompt is not a reasoning task and the bar's appeal is a first rendering inside
~2s.

**`store: false` on every request.** The Interactions API defaults to `store:
true`, retaining each interaction server-side for 55 days (1 day on free tier). A
translation utility should not leave a copy of everything typed into it on
someone else's disk by default. Nothing here uses `previous_interaction_id` or
background execution, which are the only features `store: false` costs.

**Reasoning effort is `thinking_level`, not `thinkingBudget`.** The 2.5-era
`thinkingConfig.thinkingBudget` is superseded on the 3.x line; the two cannot be
combined in one request. `low` is the default, mirroring Bedrock's `effort: low`,
and it follows the existing escape-hatch convention: setting the env var to the
empty string drops the field entirely, so the model id can point at something
that rejects it.

## Architecture

```
                      ┌─ provider/bedrock.rs ─┐
settings.json ──▶ provider/mod.rs ──▶         ├── text fragments ──▶ tones.rs
(provider, keys)  (chooses, owns    └─ provider/gemini.rs ─┘              │
                   SYSTEM_PROMPT)                                    Tone updates
                                                                          ▼
                                                                       lib.rs
```

`tones.rs` is untouched: it consumes text fragments and neither knows nor cares
which service produced them. That is the property that makes a second provider
cheap.

### provider/mod.rs

```rust
pub(crate) const SYSTEM_PROMPT: &str = "...";  // moved out of bedrock.rs

enum Provider {
    Bedrock,
    Gemini(String),  // the API key
}

/// Read per call, so a key pasted into the settings window takes effect on the
/// next translation rather than the next launch.
fn choose(app: &AppHandle) -> Provider;

pub async fn warm(app: &AppHandle) -> Result<(), String>;
pub async fn translate(app: &AppHandle, text: &str, on_delta: impl FnMut(&str))
    -> Result<(), String>;
```

Both public functions keep the signatures `lib.rs` and the example already call,
plus an `AppHandle` — that is how settings are located. `examples/translate.rs`
has no `AppHandle`, so `choose` gets a sibling taking the key directly, and the
example reads `TONEMATE_GEMINI_API_KEY` from the environment. The example is a
transport smoke test, not a settings consumer.

`provider/bedrock.rs` is today's `bedrock.rs` moved, with `SYSTEM_PROMPT` lifted
out and taken as a parameter. Its error extraction — service message first,
`DisplayErrorContext` as the transport-level fallback — is unchanged.

### provider/gemini.rs

```
POST https://generativelanguage.googleapis.com/v1beta/interactions
Content-Type: application/json
x-goog-api-key: <key>

{ "model": "gemini-3.8-flash",
  "input": "<text>",
  "system_instruction": "<SYSTEM_PROMPT>",
  "generation_config": { "thinking_level": "low" },
  "store": false,
  "stream": true }
```

Header auth is confirmed live: a bogus key returns `400 API key not valid`,
which means the header was read rather than ignored.

The response is SSE with named events. Only one carries translation text:

| Event | Handling |
| --- | --- |
| `step.delta` where `delta.type == "text"` | `delta.text` is a fragment — push to `on_delta` |
| `step.start` / `step.stop` with `step.type == "thought"` | Skip. Reasoning is a separate step type here, the analogue of filtering reasoning deltas on the Bedrock side |
| `step.delta` with `thought_summary` / `thought_signature` | Skip |
| `interaction.completed` | End of answer; carries `usage` |
| `error` | `{ message, code }` — surface `message` |
| `done` (`data: [DONE]`) | Terminate the loop |
| anything unrecognised | Log and skip; the docs promise new event and delta types over time |

Two error envelopes exist and they are not the same shape. The Interactions
endpoint returns a string code:

```json
{"error":{"message":"Your project has been denied access.","code":"permission_denied"}}
```

while `models/*` endpoints return an integer `code` plus a `status`. Only the
former matters for this file, but the parser must not assume `code` is a number.

Three distinct "HTTP 200 but no translation" cases must each produce a
distinguishable error rather than a blank result box: the stream ended without a
`model_output` step, an `error` event arrived mid-stream, and the text was
present but whitespace-only.

**Two layers of resumable buffering.** A TCP chunk can split an SSE frame
mid-line, so the frame decoder buffers until `\n\n`; `tones::Parser` then
requires the same discipline for tab and newline boundaries. The SSE decoder is
therefore written as a pure function over `&str` chunks returning text
fragments — no network, no async — which is what makes it unit-testable.

### Settings

```rust
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]   // so an existing {"accent":"indigo"} file still parses
struct Stored {
    accent: String,
    provider: String,        // "bedrock" | "gemini"; unknown → bedrock
    gemini_api_key: String,  // empty == unset
}
```

`read`/`write` are currently accent-shaped (`read -> String`,
`write(file, accent)`) and become `read(file) -> Stored` / `write(file, &Stored)`.
`accent()` and the new getters read through them. `#[serde(default)]` is the
migration guarantee: a settings file written before this change keeps working.

New commands, mirroring the `accent` / `set_accent` pair:

| Command | Returns / takes |
| --- | --- |
| `provider_config` | `{ provider, gemini_key_set }` — never the key |
| `set_provider` | provider name |
| `set_gemini_api_key` | key, trimmed; an empty string clears the stored key |

The frontend never sends an empty string on blur — an empty field means
"unchanged", and only the 清除 button clears. Rust nonetheless treats empty as
"clear", so the command has one meaning regardless of caller.

`set_provider` and `set_gemini_api_key` each spawn a background warm-up so a bad
key appears in the log at save time. They log *whether* a key is set, never its
value.

### Settings UI

```
┌─ tonemate 设置 ──────────────────┐
│  [ 外观 ]  [ 翻译服务 ]           │
├──────────────────────────────────┤
│  翻译服务                         │
│    ○ AWS Bedrock   (环境 profile) │
│    ● Google Gemini    ✓ 已配置    │
│                                  │
│    Gemini API Key                │
│    [ ····················· ] 清除 │
│                                  │
│    当前使用: Google Gemini        │
└──────────────────────────────────┘
```

The paragraph currently pointing at the README's Configuration section is
replaced by this panel. `✓ 已配置` comes from `gemini_key_set`. The key input is
`type="password"`, saves on `change` (blur or Enter) rather than per keystroke.

### Configuration

| Variable | Default | Notes |
| --- | --- | --- |
| `TONEMATE_GEMINI_MODEL` | `gemini-3.8-flash` | Bedrock keeps `TONEMATE_MODEL`; one variable cannot mean two id formats |
| `TONEMATE_GEMINI_THINKING_LEVEL` | `low` | Empty string drops the field, as `TONEMATE_EFFORT=` already does |
| `TONEMATE_GEMINI_API_KEY` | — | `examples/translate.rs` only; the GUI reads settings |

### Dependencies

`reqwest` with `default-features = false` and `json`, `stream`, `rustls-tls` —
`rustls` to avoid pulling OpenSSL into the build. `futures-util` for `StreamExt`
over `bytes_stream()`.

## Testing

The SSE decoder is the only piece with real logic and no network, so it is a pure
function and its tests come first:

- A frame split across two chunks, including a split inside `data:`
- A whole interaction — `interaction.created`, a `thought` step, a `model_output`
  step, `interaction.completed`, `done` — yielding only the answer text
- `thought_summary` and `thought_signature` deltas producing no output
- Several `step.delta` frames inside one chunk
- Blank lines and an unrecognised event name, both skipped
- An `error` event mid-stream, and a top-level error envelope with a string `code`
- A stream that ends with no `model_output` step

`settings.rs` tests extend to a round trip carrying `provider` and a key, plus
the migration case: a file containing only `accent` still reads, with `provider`
defaulting to Bedrock. `tones.rs` tests are unchanged and must stay green —
that is the regression signal for the refactor.

Then `cargo test`, `cargo clippy`, and `pnpm build` for type-checking.

**Live verification is currently blocked.** The API key available for this work
authenticates but its project is denied access — `403 permission_denied` from
both `/v1beta/interactions` and `models/gemini-3.6-flash` — so no real Gemini
translation has been performed. These remain unverified until a working key is
supplied, and must be checked before this is called done:

1. `TONEMATE_GEMINI_API_KEY=… cargo run --example translate -- "我明天不能来了"`
   streams tab-delimited rows and the parsed tone block is non-empty
2. The `generation_config` field name for an output-token cap on this endpoint
   (the documentation covers `thinking_level` and `temperature` but not a
   `max_output_tokens` equivalent); if one exists it should be set, mirroring
   Bedrock's 4096
3. Whether `gemini-3.8-flash` accepts `thinking_level: "low"`, and time to first
   word against Bedrock's ~1.8s baseline
4. That a revoked key produces a readable error in the bar, not a blank box

GUI checks are limited to launching the app and reading the log, plus rendering
`dist/settings.html` headlessly to inspect the two-panel layout: this environment
cannot send keystrokes to a window.

README's Prerequisites and Configuration sections are updated to describe both
providers and the settings panel.
