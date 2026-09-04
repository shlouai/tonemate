# tonemate

A Spotlight-style floating input bar: hit `Cmd+Shift+Space` anywhere, type text in
any language, press Enter, and the English translation is printed to the terminal
tonemate was launched from. Translation runs through Claude on Amazon Bedrock.

## Prerequisites

- [Rust toolchain](https://rustup.rs) and [pnpm](https://pnpm.io)
- AWS credentials that can call Bedrock in the configured region. By default
  tonemate uses the `twdc-bedrock-central` profile from `~/.aws/config`; if that
  profile is SSO-backed, log in first:

  ```sh
  aws sso login --profile twdc-bedrock-central
  ```

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
| `Enter` | Translate and hide the bar |
| `Esc` | Hide without translating |

**Keep the terminal visible.** The translation is printed there, streaming in as
the model produces it — there is no output surface in the bar yet:

```
[tonemate] in : 今天天气不错，我们出去走走吧。
[tonemate] out: The weather's nice today — let's go out for a walk.
```

`[tonemate] bedrock warm` appears shortly after startup. That is a background
warm-up request that pays the credential-resolution and TLS-handshake cost up
front, so the first translation is as fast as every later one (~1.8s instead of
~3.8s). If it fails, the log says why — usually expired SSO credentials.

## Translating without the GUI

To exercise the Bedrock path directly — useful for checking credentials or
timing a model change:

```sh
cd src-tauri
cargo run --example translate -- "今天天气不错，我们出去走走吧。"
```

It reports time-to-first-word and total time alongside the translation.

## Configuration

All optional; each overrides the default shown.

| Variable | Default |
| --- | --- |
| `TONEMATE_AWS_PROFILE` | `twdc-bedrock-central` |
| `TONEMATE_AWS_REGION` | `us-west-2` |
| `TONEMATE_MODEL` | `us.anthropic.claude-opus-5` |
| `TONEMATE_EFFORT` | `low` |

Bedrock serves the Claude 5 family only through cross-region inference profiles,
so `TONEMATE_MODEL` needs the `us.` prefix — a bare `anthropic.claude-opus-5` is
rejected. `TONEMATE_EFFORT` set to the empty string drops the parameter from the
request entirely, which is what lets `TONEMATE_MODEL` point at Haiku 4.5 or other
models that reject it:

```sh
TONEMATE_MODEL=us.anthropic.claude-haiku-4-5-20251001-v1:0 TONEMATE_EFFORT= pnpm tauri dev
```

## Building a release binary

```sh
pnpm tauri build
```

## Recommended IDE setup

[VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
