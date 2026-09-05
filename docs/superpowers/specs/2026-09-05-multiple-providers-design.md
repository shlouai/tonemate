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

Let the user pick the translation service in the settings window. **Kimi
(Moonshot) is the first alternative**: paste an API key, and translations go
through Kimi instead of Bedrock. Bedrock remains the default and the fallback, so
an install with no API key configured behaves exactly as it does today.

Kimi speaks the OpenAI-compatible chat-completions protocol, which DeepSeek and
OpenAI also speak. So the transport written here is not "the Kimi client" but
"the OpenAI-compatible client". Adding DeepSeek or OpenAI later needs no changes
to the transport or its twelve tests, which is the valuable part — the work sits
in the settings layer: a new key field in `Stored` and `ProviderConfig`, a new
command and registration, `choose`'s signature and tests generalised to take
multiple keys, `from_env` extended, and `src/settings.ts`'s panel logic expanded
to handle another key input (eight files total).

## Out of scope

- **Gemini.** Designed and probed, then deferred: the available key's project is
  denied access, so nothing could be verified end to end. Findings are preserved
  under [Deferred: Gemini](#deferred-gemini) so the work is not lost. Gemini needs
  its own transport (its wire format is not OpenAI-compatible), which is exactly
  why it is not bundled with this change.
- DeepSeek and OpenAI (this change makes room for them; it does not add them)
- Per-provider model pickers in the UI (model stays env-overridable)
- Showing a "key is valid" verdict in the settings window; a bad key surfaces in
  the log at save time and as an error on the first translation
- Storing the key anywhere but `settings.json` (see Decisions)
- Tool calls, multi-turn history, prompt caching

## Decisions

**An enum, not a trait.** `Provider` is an enum with one variant per service and
a two-arm `match` in `provider/mod.rs`. A trait with `async fn` returning a
stream needs boxing and a lifetime dance around the `on_delta` closure, and buys
runtime polymorphism that nothing here wants: the set of providers is fixed at
compile time and selected by an `if`. Leaving `bedrock.rs` in place and branching
in `lib.rs` was the third option, rejected because the branch would then be
duplicated in `examples/translate.rs` and `SYSTEM_PROMPT` would need two homes.

**The transport is parameterised by base URL, not by provider.** Kimi, DeepSeek
and OpenAI differ only in base URL, model id, and a few provider-specific body
fields. One `openai_compat` module takes those three as arguments. The
alternative — a file per vendor — would trebly duplicate SSE decoding, the part
with all the actual logic.

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
`{ provider, kimi_key_set: bool }`; the input renders empty with a placeholder
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

### Kimi specifics, all measured against the live API

**Reasoning must be explicitly disabled.** This is the one setting that decides
whether the provider works at all. `kimi-k2.6` defaults to
`thinking.type = "enabled"`, and reasoning tokens are drawn from the same budget
as the answer. Measured with `max_tokens: 1024` and no `thinking` field: 1023
streamed frames, all `reasoning_content`, **zero** `content` frames,
`finish_reason: length`, 21.6s elapsed — twenty-one seconds for no translation at
all. Every request therefore sends:

```json
"thinking": { "type": "disabled" }
```

`kimi-k2.6` is the only model that accepts `"disabled"`; `kimi-k2.7-code` rejects
`thinking: disabled` outright with `"invalid thinking: only type=enabled is
allowed for this model"`, which is why it is not an option here.
`reasoning_effort: "low"` is accepted without error and silently ignored (1041 vs
1201 reasoning characters), so it is not a substitute.

**`max_tokens` is deprecated; the field is `max_completion_tokens`.** Set to 4096,
matching Bedrock.

**Latency is at parity with Bedrock,** so no UX compromise is being made. Ten
successful runs with reasoning disabled:

| | First word (median) | Total (median) |
| --- | --- | --- |
| Kimi `kimi-k2.6` | ~2.4s | ~3.3s |
| Bedrock Claude Opus 5 | ~1.8s | 2.9-3.7s |

**Format compliance was 10/10** in the ten runs used to choose Kimi: five rows per
response, every row tab-delimited, Chinese labels, direction correct both ways.

**That sample was too small, and the conclusion drawn from it — that the existing
`SYSTEM_PROMPT` needed no change — was wrong.** Measured later over 20 runs
across eight inputs, the original prompt produced a wrong-direction answer in 6
and labels that paraphrased the input rather than naming a tone in 7. `kimi-k2.6`
also pins `temperature` (0.6 with reasoning off, 1.0 with it on) and rejects any
other value, so the variance cannot be damped from our side; the prompt is the
only lever. It was rewritten to state the direction rule first as a prohibition,
to define the label as naming a tone and never a paraphrase, and to show the
separator as a real tab in a worked example instead of naming it in backslash-t
notation — which Kimi sometimes emitted literally, welding the label onto the
text. 40 consecutive runs then came back clean on all three counts.

The lesson worth keeping: Claude tolerated a prompt whose direction rule was one
clause among many. A shared prompt is only as good as the least instruction-
adherent model behind it, so format compliance needs measuring per provider at a
sample size that can actually see a 30% failure rate.

**There is no problematic rate limit.** An earlier probe hit `429` on
back-to-back calls, which suggested the startup warm-up could collide with a
user's first translation. That was an artifact of a suspended account, not the
API: retested, four back-to-back calls, a warm-up immediately followed by a real
call, and two concurrent calls all succeeded. The warm-up needs no special
handling.

**Two hosts, and a key works with exactly one.** `api.moonshot.cn` (domestic) and
`api.moonshot.ai` (international) both exist; a key valid on one returns `401
Invalid Authentication` on the other — confirmed with two different keys, one for
each host. A user cannot be expected to know which they hold from the key string,
so the base URL is env-overridable and the error message must be quotable enough
to make `401` diagnosable. Default is `https://api.moonshot.cn/v1`.

## Architecture

```
                      ┌─ provider/bedrock.rs ───────┐
settings.json ──▶ provider/mod.rs ──▶               ├─ text fragments ─▶ tones.rs
(provider, keys)  (chooses, owns    └─ provider/openai_compat.rs ┘            │
                   SYSTEM_PROMPT)      (Kimi now; DeepSeek,              Tone updates
                                        OpenAI later)                         ▼
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
    Kimi(String),  // the API key
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
example reads `TONEMATE_KIMI_API_KEY` from the environment. The example is a
transport smoke test, not a settings consumer.

`provider/bedrock.rs` is today's `bedrock.rs` moved, with `SYSTEM_PROMPT` lifted
out and taken as a parameter. Its error extraction — service message first,
`DisplayErrorContext` as the transport-level fallback — is unchanged.

### provider/openai_compat.rs

```rust
pub struct Endpoint {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// Provider-specific body fields merged into the request. Kimi supplies
    /// {"thinking": {"type": "disabled"}}; DeepSeek and OpenAI will supply
    /// their own, or nothing.
    pub extra: serde_json::Value,
}
```

```
POST {base_url}/chat/completions
Content-Type: application/json
Authorization: Bearer {api_key}

{ "model": "kimi-k2.6",
  "stream": true,
  "max_completion_tokens": 4096,
  "thinking": { "type": "disabled" },
  "messages": [ { "role": "system", "content": "<SYSTEM_PROMPT>" },
                { "role": "user",   "content": "<text>" } ] }
```

The response is SSE, one JSON object per `data:` line:

| Field | Handling |
| --- | --- |
| `choices[].delta.content` | A text fragment — push to `on_delta` |
| `choices[].delta.reasoning_content` | **Skip.** Present in streaming deltas despite being undocumented there; the analogue of filtering reasoning deltas on the Bedrock side |
| `choices[].delta.role` | Ignore (the first frame carries `role` with empty content) |
| `choices[].finish_reason` | Retain for the empty-output error message; `length` means the budget ran out |
| `data: [DONE]` | Terminate the loop |
| `{"error": {...}}` as the whole body | Non-2xx; surface `error.message` |

Error bodies are `{"error":{"message":…,"type":…}}` — for example
`invalid_authentication_error` for the wrong host, and
`exceeded_current_quota_error` for an unpaid account. `message` is the useful
part and is surfaced verbatim, prefixed with the provider name.

Three distinct "HTTP 200 but no translation" cases must each produce a
distinguishable error rather than a blank result box: the stream carried only
`reasoning_content`, `finish_reason` was `length` with no content, and the text
was present but whitespace-only.

**Two layers of resumable buffering.** A TCP chunk can split an SSE frame
mid-line, so the frame decoder buffers until a newline; `tones::Parser` then
requires the same discipline for tab and newline boundaries. The SSE decoder is
therefore written as a pure function over `&str` chunks returning text
fragments — no network, no async — which is what makes it unit-testable.

### Settings

```rust
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]   // so an existing {"accent":"indigo"} file still parses
struct Stored {
    accent: String,
    provider: String,      // "bedrock" | "kimi"; unknown → bedrock
    kimi_api_key: String,  // empty == unset
}
```

`read`/`write` are currently accent-shaped (`read -> String`,
`write(file, accent)`) and become `read(file) -> Stored` / `write(file, &Stored)`.
`accent()` and the new getters read through them. `#[serde(default)]` is the
migration guarantee: a settings file written before this change keeps working.
No Gemini field is added — an unused key in the on-disk format would be a
promise this change does not keep.

New commands, mirroring the `accent` / `set_accent` pair:

| Command | Returns / takes |
| --- | --- |
| `provider_config` | `{ provider, kimi_key_set }` — never the key |
| `set_provider` | provider name |
| `set_kimi_api_key` | key, trimmed; an empty string clears the stored key |

The frontend never sends an empty string on blur — an empty field means
"unchanged", and only the 清除 button clears. Rust nonetheless treats empty as
"clear", so the command has one meaning regardless of caller.

`set_provider` and `set_kimi_api_key` each spawn a background warm-up so a bad
key appears in the log at save time. They log *whether* a key is set, never its
value.

### Settings UI

```
┌─ tonemate 设置 ──────────────────┐
│  [ 外观 ]  [ 翻译服务 ]           │
├──────────────────────────────────┤
│  翻译服务                         │
│    ○ AWS Bedrock   (环境 profile) │
│    ● Kimi             ✓ 已配置    │
│                                  │
│    Kimi API Key                  │
│    [ ····················· ] 清除 │
│                                  │
│    当前使用: Kimi                 │
└──────────────────────────────────┘
```

The paragraph currently pointing at the README's Configuration section is
replaced by this panel. `✓ 已配置` comes from `kimi_key_set`. The key input is
`type="password"`, saves on `change` (blur or Enter) rather than per keystroke.

### Configuration

| Variable | Default | Notes |
| --- | --- | --- |
| `TONEMATE_KIMI_BASE_URL` | `https://api.moonshot.cn/v1` | Set to `https://api.moonshot.ai/v1` for an international key; the wrong host returns `401` |
| `TONEMATE_KIMI_MODEL` | `kimi-k2.6` | Bedrock keeps `TONEMATE_MODEL`; one variable cannot mean two id formats. `kimi-k2.7-code` will not work — it forces reasoning on |
| `TONEMATE_KIMI_API_KEY` | — | `examples/translate.rs` only; the GUI reads settings |

### Dependencies

`reqwest` with `default-features = false` and `json`, `stream`, `rustls-tls` —
`rustls` to avoid pulling OpenSSL into the build. `futures-util` for `StreamExt`
over `bytes_stream()`.

## Testing

The SSE decoder is the only piece with real logic and no network, so it is a pure
function and its tests come first:

- A frame split across two chunks, including a split inside `data:`
- The realistic opening frame — `delta: {role: "assistant", content: ""}` —
  producing no output
- `reasoning_content` deltas producing no output, interleaved with `content`
- Several `data:` frames inside one chunk
- Blank lines and `data: [DONE]`
- An error body instead of a stream, including both observed `type` values
- A stream whose only deltas were `reasoning_content`, and one ending with
  `finish_reason: "length"` and no content — each a distinguishable error

`settings.rs` tests extend to a round trip carrying `provider` and a key, plus
the migration case: a file containing only `accent` still reads, with `provider`
defaulting to Bedrock. `tones.rs` tests are unchanged and must stay green —
that is the regression signal for the refactor.

Then `cargo test`, `cargo clippy`, and `pnpm build` for type-checking.

Live verification, with a working key:

1. `TONEMATE_KIMI_API_KEY=… cargo run --example translate -- "我明天不能来了"`
   streams tab-delimited rows; first word within ~2.5s, total within ~3.5s
2. Selecting Kimi with no key stored falls back to Bedrock and says so in the log
3. A wrong-host or revoked key produces a readable, provider-prefixed error in
   the bar, not a blank box

GUI checks are limited to launching the app and reading the log, plus rendering
`dist/settings.html` headlessly to inspect the two-panel layout: this environment
cannot send keystrokes to a window.

README's Prerequisites and Configuration sections are updated to describe both
providers and the settings panel.

## Deferred: Gemini

Kept because it was established by probing the live API, contradicts the public
documentation, and would otherwise be rediscovered from scratch.

- **`streamGenerateContent` is closed to new projects.**
  `models/gemini-2.5-flash:streamGenerateContent` returns HTTP 404: *"This model
  models/gemini-2.5-flash is no longer available to new users. Please update your
  code to use models/gemini-3.6-flash… We recommend you to use the Interactions
  API."* `ListModels` returns 54 models, of which **zero** advertise
  `streamGenerateContent`.
- **`ai.google.dev` still documents `gemini-2.5-flash` as active.** The API
  disagrees. The API wins.
- **The path forward is `POST /v1beta/interactions`** with `stream: true`, header
  auth via `x-goog-api-key` (confirmed: a bogus key returns `400 API key not
  valid`, so the header is read), `store: false` to avoid 55-day server-side
  retention of everything typed into the app, and
  `generation_config.thinking_level: "low"` — the 2.5-era
  `thinkingConfig.thinkingBudget` is superseded and the two cannot be combined.
- **Its SSE format is not OpenAI-compatible:** named events (`step.start`,
  `step.delta`, `step.stop`, `interaction.completed`, `done`), answer text at
  `delta.text` where `delta.type == "text"` inside a `model_output` step, and
  reasoning as a separate `thought` step type. It needs its own decoder, so it
  cannot reuse `openai_compat.rs`.
- **Two error envelope shapes:** `/v1beta/interactions` returns a *string* `code`
  (`"permission_denied"`), while `models/*` routes return an integer `code` plus
  a `status`. A parser must not assume `code` is a number.
- **Blocked on billing, not code.** Both `/v1beta/interactions` and
  `/v1/models/gemini-3.8-flash:generateContent` return `403 Your project has been
  denied access`, while a bogus key returns `400 API key not valid` — so the key
  authenticates and the project behind it is blocked.
- **Unresolved:** whether `generation_config` has a `max_output_tokens`
  equivalent on this endpoint. The documentation covers `thinking_level` and
  `temperature` only. Bedrock caps at 4096; without an equivalent, long inputs
  have no truncation guard.
