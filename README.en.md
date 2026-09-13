# tonemate

[简体中文](README.md) · English

> Hit a shortcut, type a sentence, and get the same sentence back in several different "tones" — pick the one that fits who you're sending it to.

![tonemate demo](assets/demo.gif)

## What is this?

tonemate is a small translation tool that lives in the menu bar (system tray on Windows / Linux). It stays out of the way until you need it: press a shortcut and a floating input bar appears over whatever app you're in.

1. Type a sentence and press Enter.
2. It translates the sentence into 3–5 different tones, streaming them in line by line, each labelled (blunt, polite, formal, courteous…).
3. Pick the one that fits your reader, and click it to copy.

The direction is detected automatically:

- **Chinese** in → **English** out;
- **English** in → **Chinese** out;
- anything else in → **English** out.

The first line is always the most literal, most neutral rendering; later lines move further from it in tone, so you can compare and choose.

## Screenshots

| Input bar | Multi-tone results |
| --- | --- |
| ![Input bar](assets/screenshots/1-input-bar.png) | ![Multi-tone results](assets/screenshots/2-tones.png) |

| Accent colours | Translation service |
| --- | --- |
| ![Accent colours](assets/screenshots/3-accent.png) | ![Translation service](assets/screenshots/4-provider.png) |

## Install

Download the latest release (currently v0.1.0) from [Releases](https://github.com/shlouai/tonemate/releases):

| Platform | Package | Notes |
| --- | --- | --- |
| macOS | `tonemate_0.1.0_universal.dmg` | Open the DMG and drag tonemate into Applications |
| macOS (no install) | `tonemate_0.1.0_universal.app.zip` | Unzip and run tonemate.app directly |
| Windows | `tonemate_0.1.0_x64-setup.exe` | Run the installer, then launch from the Start menu / desktop |
| Windows (portable) | `tonemate_0.1.0_x64_portable.zip` | Unzip and double-click tonemate.exe |

The macOS build is a universal binary (Intel + Apple Silicon); the Windows build relies on the WebView2 runtime, which ships with Windows 10/11.

> **First launch on macOS**: the app is unsigned, so Gatekeeper blocks it. Right-click the app → **Open**, or approve it under **System Settings → Privacy & Security → Open Anyway**.

A `SHA256SUMS.txt` with checksums is included in the release for verifying downloads.

The window starts hidden — only a new icon appears in the menu bar / tray. Press the shortcut to summon the bar (see below).

## How to use it

### Run from source (developers)

To run from source instead of the packaged build:

```sh
pnpm install
pnpm tauri dev
```

The window starts hidden — only a new icon appears in the menu bar / tray. Once the terminal prints `[tonemate] hotkey registered: ...`, it's live.

### Summon the bar

| Key | Action |
| --- | --- |
| `Cmd+Shift+Space` (macOS)<br>`Ctrl+Shift+Space` (Windows / Linux) | Show the bar; press again to hide |
| `Enter` | Translate, keeping the bar up to show the result |
| `Esc` | Hide and clear the result |

### Type, translate, copy

Summon the bar, type, and press Enter. The renderings appear line by line in real time; **click any line** to copy it (without its tone label) to the clipboard.

Clicking the menu bar / tray icon opens a three-item menu: summon the bar, open settings, and quit.

## Settings

Click the icon → **Settings…** to open the settings window, which has two panels:

- **Appearance**: change the bar's colour (graphite / indigo / pine / wine / violet / amber); the change applies immediately.
- **Translation service**: pick the service and enter its credential.

## Translation services

tonemate uses **AWS Bedrock** by default; you can switch to another service in settings:

| Service | What it needs |
| --- | --- |
| AWS Bedrock | Default. Needs AWS credentials on this machine that can call Bedrock |
| DeepSeek | An API key ([platform.deepseek.com](https://platform.deepseek.com)) |
| Qwen | An API key ([Model Studio](https://www.alibabacloud.com/help/en/model-studio)) |
| Kimi | An API key ([platform.moonshot.cn](https://platform.moonshot.cn)) |

Choosing Kimi / DeepSeek / Qwen without a saved key falls back to Bedrock and says so in the log; a key the service *rejects* is reported as an error instead of silently falling back — otherwise a revoked key would keep billing AWS.

### Local model (offline translation)

Pick **Local model (Hy-MT2)** in Settings → Translation service to translate offline, with your text never leaving this machine.
Selecting it for the first time downloads the model automatically (~440 MB, via the `hf-mirror.com` mirror); the download is resumable.
Translation runs locally through llama.cpp, which prepares its runtime on the first translation (a few seconds). Both the model and the runtime fall back to mirrors automatically when their official host is unreachable — no manual configuration needed.

The default is still AWS Bedrock; nothing is downloaded unless you select the local model.

Available environment variables:

| Variable | Default | Notes |
| --- | --- | --- |
| `TONEMATE_LOCAL_MODEL_URL` | built-in mirror URL | Override the model download URL |
| `TONEMATE_LLAMA_SERVER_URL` | llama.cpp b10936 release URL | Override the runtime download URL (optional; mirrors are tried automatically) |
| `TONEMATE_LOCAL_PORT` | `8931` | llama-server port |
| `TONEMATE_LOCAL_N_GPU_LAYERS` | `0` | GPU offload layers; 0 = CPU only |

---

## Advanced (developers / deep configuration)

> Ordinary users can stop here. The rest is for anyone building from source or tuning the model parameters.

### Build a release binary

```sh
pnpm tauri build
```

### Translate from the command line (no GUI)

To exercise the translation path directly — useful for checking credentials or timing a model change:

```sh
cd src-tauri
cargo run --example translate -- "今天天气不错，我们出去走走吧。"
```

It prints the raw stream, each parsed tone, and time-to-first-word / total time. The example uses Bedrock unless one of `TONEMATE_KIMI_API_KEY` / `TONEMATE_DEEPSEEK_API_KEY` / `TONEMATE_QWEN_API_KEY` is set.

### Environment variables

The settings window holds only the choices that belong to a person (colour, service, key, AWS profile), and they take effect immediately. Everything else is environment-only — all optional, each overriding the default shown below.

| Variable | Default | Applies to |
| --- | --- | --- |
| `TONEMATE_AWS_PROFILE` | — | Bedrock |
| `TONEMATE_AWS_REGION` | `us-west-2` | Bedrock |
| `TONEMATE_MODEL` | `us.anthropic.claude-opus-5` | Bedrock |
| `TONEMATE_EFFORT` | `low` | Bedrock |
| `TONEMATE_KIMI_BASE_URL` | `https://api.moonshot.cn/v1` | Kimi |
| `TONEMATE_KIMI_MODEL` | `kimi-k2.6` | Kimi |
| `TONEMATE_DEEPSEEK_BASE_URL` | `https://api.deepseek.com/v1` | DeepSeek |
| `TONEMATE_DEEPSEEK_MODEL` | `deepseek-chat` | DeepSeek |
| `TONEMATE_QWEN_BASE_URL` | `https://dashscope.aliyuncs.com/compatible-mode/v1` | Qwen |
| `TONEMATE_QWEN_MODEL` | `qwen3.7-plus` | Qwen |

A few notes:

- **Bedrock model**: Bedrock serves the Claude 5 family only through cross-region inference profiles, so `TONEMATE_MODEL` needs the `us.` prefix — a bare `anthropic.claude-opus-5` is rejected. Setting `TONEMATE_EFFORT` to the empty string drops the parameter entirely, which lets the model point at Haiku 4.5 and other models that reject it:

  ```sh
  TONEMATE_MODEL=us.anthropic.claude-haiku-4-5-20251001-v1:0 TONEMATE_EFFORT= pnpm tauri dev
  ```

- **Kimi**: Moonshot runs a domestic host (`api.moonshot.cn`) and an international one (`api.moonshot.ai`); a key issued for one returns `401` on the other, and the key doesn't say which it is — so a `401` usually means the wrong `TONEMATE_KIMI_BASE_URL`. Keep the model on `kimi-k2.6`: it's the only Kimi model whose reasoning can be switched off; `kimi-k2.7-code` rejects `thinking: disabled` and fails immediately.

- **DeepSeek**: keep the model on `deepseek-chat` (the non-reasoning model). `deepseek-reasoner` is a reasoning model and would spend the whole budget deliberating without returning a translation.

- **Qwen**: also has a domestic host (`dashscope.aliyuncs.com`) and an international one (`dashscope-intl.aliyuncs.com`); using the key on the wrong host returns `401`. The model defaults to `qwen3.7-plus`; whichever you pick, thinking must stay off (the app already sends `enable_thinking: false`) — otherwise it reasons for ~21s before the first word.

### Where settings are stored

Settings live in `~/Library/Application Support/com.tonemate.app/settings.json` (`%APPDATA%\com.tonemate.app\settings.json` on Windows, `~/.config/com.tonemate.app/settings.json` on Linux) and survive a restart. **API keys are stored there in cleartext**, readable by anything running as you; the file is written owner-only, which is a speed bump rather than protection.

### Recommended IDE setup

[VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
