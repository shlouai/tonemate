# tonemate

A Spotlight-style floating input bar: hit `Cmd+Shift+Space` anywhere, type text in
any language, press Enter, and several translations stream into a box under the
input — the same sentence rendered in 3 to 5 different tones, each labelled, so
you can pick the one that fits who is reading it. The direction picks itself —
English in gets Chinese back, anything else gets English. Translation runs through
Claude on Amazon Bedrock by default, or through Kimi or DeepSeek if you paste an
API key into the settings window.

## Prerequisites

- [Rust toolchain](https://rustup.rs) and [pnpm](https://pnpm.io)
- Credentials for one of the two translation services:
  - **AWS Bedrock** (the default) — credentials that can call Bedrock in the
    configured region.

  - **Kimi** — an API key from [platform.moonshot.cn](https://platform.moonshot.cn),
    pasted into the settings window. Nothing else to install.

  - **DeepSeek** — an API key from [platform.deepseek.com](https://platform.deepseek.com),
    pasted into the settings window. Nothing else to install.

## Running it

```sh
pnpm install
pnpm tauri dev
```

The window starts hidden — there is nothing to see until you summon it. Once
`[tonemate] hotkey registered: Cmd+Shift+Space` appears, the app is live:

| Key | Action |
| --- | --- |
| `Cmd+Shift+Space` | Show the bar (or hide it, if it is already up) |
| `Enter` | Translate, keeping the bar up to show the result |
| `Esc` | Hide, clearing the result |

tonemate lives in the menu bar and not in the Dock — its icon in the right-hand
end of the status bar is the only part of it you can point at. Clicking it opens
a three-item menu: summon the bar (the same thing the hotkey does), open the
settings window, and quit. The settings window has two panels: 外观 holds the
bar's colour, and 翻译服务 picks the translation service, holds its API key, and
holds the AWS profile Bedrock should use. Model ids and hosts are still set
through the environment — see [Configuration](#configuration).

The result box under the input is hidden until there is something to show, then
grows the window downwards as the renderings stream in. How many you get is the
model's call: it reads what the input is trying to accomplish and gives 3 to 5
renderings that genuinely differ in tone, rather than padding to a fixed count.
The first is always the most literal. The text can be selected and copied;
anything longer than the box scrolls. The same exchange is still logged to the
launching terminal:

```
[tonemate] in : 我明天不能来了
[tonemate] out:
直白	I can't come tomorrow.
委婉	I'm afraid I won't be able to make it tomorrow.
正式	I regret to inform you that I will be unable to attend tomorrow.
客气	Sorry, something's come up and I won't be able to come by tomorrow.
冷淡	Not coming tomorrow.
```

`[tonemate] bedrock warm` — or `[tonemate] kimi warm` / `[tonemate] deepseek warm`,
naming whichever service is selected — appears shortly after startup. That is a
background warm-up request that pays the one-time setup cost up front, so it does
not land on your first translation. On Bedrock that cost is credential resolution
plus the TLS handshake, and takes 2-3s; Kimi and DeepSeek have no credential chain
to resolve, so it is just the handshake. If it fails, the log says why: expired
SSO credentials on Bedrock, and on Kimi or DeepSeek usually a rejected API key or
the wrong host.

Warm, Bedrock's first rendering appears about 1.8s after you press Enter and the
full set totals 2.9-3.7s. Kimi is comparable and often quicker — measured at
under 1s to the first word and about 2.3s in total.

## Translating without the GUI

To exercise the translation path directly — useful for checking credentials or
timing a model change:

```sh
cd src-tauri
cargo run --example translate -- "今天天气不错，我们出去走走吧。"
```

It prints the raw stream, then a `[tonemate] parsed N tones:` block listing each
labelled rendering, then time-to-first-word and total time. It uses Bedrock
unless `TONEMATE_KIMI_API_KEY` or `TONEMATE_DEEPSEEK_API_KEY` is set, since it has
no access to the settings window's choice:

```sh
TONEMATE_KIMI_API_KEY=sk-… cargo run --example translate -- "我明天不能来了"
TONEMATE_DEEPSEEK_API_KEY=sk-… cargo run --example translate -- "我明天不能来了"
```

## Configuration

The settings window holds the two choices that belong to a person rather than to
a machine, and both take effect immediately:

- **输入条颜色** — 石墨, 靛蓝, 墨绿, 酒红, 紫罗兰 or 琥珀, and the bar repaints as
  you choose. All six are dark panes of the same lightness, because the text,
  borders and grip drawn on them are white at some opacity; a light bar would be
  a second theme rather than a colour.
- **翻译服务** — AWS Bedrock, Kimi, or DeepSeek. Choosing Kimi or DeepSeek
  without saving a key falls back to Bedrock and says so, since an empty key
  means "not set up yet". A key that the service *rejects* is reported instead of
  falling back — otherwise a revoked key would silently bill AWS forever.
- **AWS Profile** — the profile Bedrock should use. Leave it empty and Bedrock
  falls back to `TONEMATE_AWS_PROFILE`, then to AWS's own default chain
  (`AWS_PROFILE`, or the `default` profile in `~/.aws/config`). Unlike the keys,
  the profile name is not a secret, so the field shows it in full.

These live in
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
| `TONEMATE_DEEPSEEK_BASE_URL` | `https://api.deepseek.com/v1` | DeepSeek |
| `TONEMATE_DEEPSEEK_MODEL` | `deepseek-chat` | DeepSeek |
| `TONEMATE_DEEPSEEK_API_KEY` | — | the CLI example only |

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
switched off. `kimi-k2.7-code` rejects `thinking: disabled` and fails
immediately. With reasoning left on for `kimi-k2.6`, the model spends the entire
token budget deliberating and returns no translation at all (21.6s, finish
reason `length`, zero content).

One DeepSeek note, of the same shape: keep `TONEMATE_DEEPSEEK_MODEL` on
`deepseek-chat`, the non-reasoning model. `deepseek-reasoner` is deliberately not
offered — like Kimi's reasoning, it would spend the budget deliberating and
return no translation.

## Building a release binary

```sh
pnpm tauri build
```

## Recommended IDE setup

[VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
