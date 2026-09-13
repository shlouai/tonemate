# Local LLM (Hy-MT2) translation

## Problem

tonemate translates through cloud providers only (Bedrock, Kimi, DeepSeek,
Qwen). Every path needs either an AWS profile or an API key, and every
translation sends the user's text to a third party. There is no way to translate
offline, and no way to keep text on the machine.

The provider seam is already clean — `Provider` is an enum, the
OpenAI-compatible transport is parameterised by base URL, and `tones.rs`
consumes text fragments without knowing who produced them. What is missing is a
*local* provider: a GGUF model running on the machine, plus the machinery to
fetch it and run it.

## Goal

Let the user pick a local model in the settings window. The model is Tencent's
[Hy-MT2-1.8B](https://github.com/Tencent-Hunyuan/Hy-MT2), a multilingual
translation model, in its 1.25-bit GGUF form (`Hy-MT2-1.8B-1.25Bit.gguf`,
461,860,800 bytes ≈ 440 MB). Bedrock remains the default; local is opt-in.
Selecting local with no model file present starts the download automatically.
Translations then run through a local llama.cpp server, reusing the existing
streaming transport.

## Out of scope

- Bundling the model weights into the app — always downloaded on demand.
- Bundling the llama-server binary — downloaded on demand (see Decisions).
- GPU-offload tuning (Metal / CUDA). First version runs CPU-only, which is ample
  for a 1.8B model; the `-ngl` knob is exposed via env for anyone who wants it.
- Arbitrary GGUF / multi-model pickers — one pinned model.
- Showing download progress in the main bar — progress lives in the settings
  window, which is where the selection that triggers it happens.
- A "translation quality" verdict for the local model in the UI.

## Decisions

**llama-server as a sidecar, not an embedded engine.** The model's architecture
is `hunyuan-dense` and its quantization is 1.25-bit; both are recent llama.cpp
features that only llama.cpp supports. Embedding llama.cpp via the
`llama-cpp-2` crate would drag a cmake + C++ toolchain into a build that already
cross-compiles for macOS universal, and would pin us to whatever llama.cpp
version the crate bundles. `candle` (pure Rust) is ruled out: it supports
neither `hunyuan-dense` nor 1.25-bit. A sidecar `llama-server` keeps the Rust
build pure, always runs a llama.cpp recent enough for the model, and — because
`llama-server` speaks the OpenAI-compatible `/v1/chat/completions` protocol —
lets the local provider reuse `provider/openai_compat.rs` unchanged. The chat
template is applied by `llama-server` from the GGUF metadata, so no Hunyuan
templating is written by hand.

**The binary is downloaded at runtime, like the model.** Both the model (440 MB)
and the `llama-server` binary (~20–50 MB) are fetched on demand into the app's
data directory. This keeps the build pure Rust and sidesteps Tauri's `externalBin`
handling for macOS universal, which is fiddly and cannot be verified from this
development machine. The cost — first local use needs a network connection — is
already paid by the model download, so bundling the binary would buy only a
smaller first fetch at real build complexity. Consequences handled below:
the downloaded binary is `chmod +x`-ed on Unix, and its URL is env-overridable
for mirrors.

**The server is loaded lazily, not on selection.** Selecting local starts the
*download*; the 440 MB model is not loaded into RAM until the first translation.
This is the difference between a settings click and a multi-hundred-MB load. The
startup and `set_provider` warm-up therefore do a cheap presence check for local
rather than spinning up a server. (See `warm` below.)

**Local has no key and no fallback.** Unlike Kimi/DeepSeek/Qwen, "local" needs no
credential, so the two-failure-mode table from the multiple-providers work does
not apply: there is no "selected but unconfigured" state to fall back from. The
only analogous failure — model not downloaded — is a real error surfaced in the
bar, not a silent fallback to Bedrock, because silently billing AWS when the user
asked for offline translation is exactly the failure the fallback rule exists to
avoid in the other direction.

**Reuse the shared prompt; verify, then adapt.** The local provider sends the
same `prompt_for` as the cloud providers. `tones::Parser` already renders a
single unlabelled line as one unlabelled row, so Hy-MT2's native single-faithful-
translation output degrades gracefully. Whether it follows the 3–5 tonal-variant
instruction is measured during implementation against the downloaded model; if it
does not, the local provider switches to a plain single-translation prompt. This
is a prompt-string change, not an architecture change, so it is deliberately left
as an implementation-time verification rather than pre-decided.

**The download is resumable and reports progress via events.** The file streams
to `models/<name>.part` and is renamed on completion, so a partial download
survives restart and resumes with a `Range` header. Progress is emitted as
`model:download-progress` (bytes/total) to the settings window, which is the only
window that needs it.

**One pinned model, one pinned llama.cpp release.** The model filename, size, and
a llama.cpp release version are constants, not user input. This is what makes
resume, checksum verification, and per-platform binary URLs tractable.

## Architecture

```
settings.json ──▶ provider/mod.rs ──▶ provider/local.rs ──┐
(provider=local)   (chooses, owns      (model path,        │
                    SYSTEM_PROMPT)      download, server)  │
                                             │             ▼
                                     openai_compat.rs ── llama-server ── GGUF
                                     (unchanged, base_url = 127.0.0.1:8931/v1)
```

`tones.rs` is untouched. `provider/bedrock.rs`, `provider/sse.rs`, and the
Kimi/DeepSeek/Qwen arms of `provider/mod.rs` are untouched.

### provider/local.rs (new)

- **Path resolution** — the model lives at
  `app_data_dir()/models/Hy-MT2-1.8B-1.25Bit.gguf`; the binary at
  `app_data_dir()/bin/llama-server(.exe)`.
- **Download** — a single streaming downloader parameterised by URL and
  destination, used for both the model and the binary. It opens the destination
  `.part` in append mode, sends `Range: bytes=<len>-`, streams the response to
  disk counting bytes, and renames to the final name once the full size arrives.
  Final-size check (461,860,800 for the model) is the minimum integrity guard;
  SHA-256 verification against Hugging Face's checksum is a follow-up if the
  mirror cannot be trusted.
- **Server lifecycle** — spawns `llama-server` via `std::process::Command` with
  `--model <gguf> --host 127.0.0.1 --port <port> --ctx-size 8192` (and
  `--n-gpu-layers` from env, default off). The `Child` handle is held in a
  process-lifetime static; a spawn is skipped when `/health` already answers on
  the port (a stale server from a crash is reused rather than fought). The child
  is killed on app exit.
- **`ensure_ready(app) -> Result<Endpoint, String>`** — the single entry point
  the translation path calls: model present? binary present? server healthy
  (poll `/health` until it answers or a timeout)? Then return the
  `openai_compat::Endpoint`. Any missing piece becomes a readable error ("本地模型
  尚未下载完成…", "llama-server 启动失败…"), never a blank box.

### provider/mod.rs

- `enum Provider` gains `Local` (a unit variant — path and server are resolved
  lazily, not held).
- `choose` gains a `"local" => Provider::Local` arm, no fallback.
- `label()` returns `"local"`.
- `warm(Local)` checks the model file exists and returns; it does **not** spawn
  or load. `translate(Local)` calls `local::ensure_ready` then
  `openai_compat::converse` against the returned endpoint with
  `max_tokens_field: "max_tokens"`, empty `api_key`, empty `extra`.

### settings.rs

- `DEFAULT_PROVIDER` stays `"bedrock"`; no new field is stored — the model's
  presence is derived from the filesystem, not remembered.
- `ProviderConfig` gains `local_model_state: String` — one of `"downloaded"`,
  `"missing"`, `"downloading"`, `"error"` — and `local_model_size: u64` (bytes on
  disk, or 0).
- `set_provider("local")` spawns `provider::local::ensure_downloaded`, which
  starts the download if the model is missing and emits the events below. This is
  the "selecting local auto-starts the download" behaviour, and it is the same
  shape as the existing spawned warm-up: the settings window does not block.

### Events (settings window)

| Event | Payload |
| --- | --- |
| `model:download-progress` | `{ bytes, total }` |
| `model:download-done` | `{ path }` |
| `model:download-error` | `{ message }` |

### Settings UI (settings.html + settings.ts)

- A fourth provider radio, value `local`, labelled 本地模型 (Hy-MT2).
- A `data-provider="local"` panel (in the same `.key`-slot pattern) showing the
  download state, a progress bar driven by `model:download-progress`, and the
  on-disk size. The 未下载 state offers nothing to type — it just reports, since
  selection is what starts the download.
- `wireProvider`'s `render` maps `local_model_state` to the panel text and adds
  `"local"` to the name/label resolution in the 当前使用 line.

### lib.rs

- `settings::set_provider` and the new commands are registered as before.
- An `ExitRequested` handler (alongside the existing settings-hide handler) kills
  the llama-server child so it does not outlive the app.

## Configuration

| Variable | Default | Notes |
| --- | --- | --- |
| `TONEMATE_LOCAL_MODEL_URL` | hf-mirror → huggingface.co | The GGUF URL, tried in order; mirrors are how the model is reached from this network |
| `TONEMATE_LLAMA_SERVER_URL` | llama.cpp GitHub release | Per-platform binary URL (or a `{os}/{arch}` template); env-overridable for mirrors |
| `TONEMATE_LOCAL_PORT` | `8931` | llama-server's port; changed only if 8931 collides |
| `TONEMATE_LOCAL_N_GPU_LAYERS` | `0` | `--n-gpu-layers`; 0 = CPU-only |

The model URL defaults to `https://hf-mirror.com/tencent/Hy-MT2-1.8B-1.25Bit-GGUF/resolve/main/Hy-MT2-1.8B-1.25Bit.gguf`
with `https://huggingface.co/…` as the fallback, because `huggingface.co` is not
reachable from the development machine.

## Dependencies

- `reqwest` already has `stream` + `rustls-tls`; nothing new is needed for
  downloads.
- No `tauri-plugin-shell`: the binary is downloaded, not an `externalBin`
  sidecar, so it is spawned with `std::process::Command` and polled with the
  existing `reqwest` client.
- No new Rust crates. `std::process::Command` covers spawn/kill; `reqwest`
  covers download and the `/health` poll.

## Testing

Pure, unit-testable pieces first:

- `settings.rs` — `provider = "local"` round-trips; a pre-provider file still
  reads with the default; `provider_config` reports `local_model_state` from the
  filesystem (a temp dir stands in for `app_data_dir`).
- `provider/mod.rs` — `choose("local", …)` yields `Provider::Local` with no key
  and no fallback; `label()`.
- The downloader's `Range`/resume arithmetic as a pure function (existing length,
  response status → offset to resume from, or start over).

Then `cargo test`, `cargo clippy`, and `pnpm build` for type-checking.

Live verification on Windows (the development machine):

1. `cargo run --example translate -- "我明天不能来了"` with the local provider
   path exercised via env, streaming tab-delimited rows.
2. Select 本地模型 with no model file → download starts and the progress bar
   fills; interrupting and relaunching resumes rather than restarting.
3. After download, a translation runs through `127.0.0.1:8931` and streams rows;
   a fresh translation reuses the running server.
4. Selecting Bedrock again leaves the local model downloaded but idle.

macOS behaviour is exercised through the existing `pnpm build:mac` flow, with the
manual translation checks above run on a Mac as the release target allows.

## Open questions carried into implementation

- Whether Hy-MT2 follows the shared tonal-variant prompt, or the local provider
  needs the single-translation prompt (decided by probing the downloaded model).
- The exact pinned llama.cpp release and its per-platform asset URLs (chosen at
  implementation time as the latest release with `hunyuan-dense` + 1.25-bit).
