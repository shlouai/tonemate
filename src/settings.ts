import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

const appWindow = getCurrentWindow();

/** What Rust will say about the translation service. Never the key itself, but
    the AWS profile is not a secret, so it comes through in full. */
type ProviderConfig = {
  provider: string;
  kimi_key_set: boolean;
  deepseek_key_set: boolean;
  qwen_key_set: boolean;
  aws_profile: string;
  local_model_state: string;
  local_model_size: number;
};

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

  let config = await invoke<ProviderConfig>("provider_config").catch(
    (error): ProviderConfig => {
      console.error(error);
      // Bedrock needs nothing configured, so it is the safe thing to show when
      // the question could not be asked.
      return {
        provider: "bedrock",
        kimi_key_set: false,
        deepseek_key_set: false,
        qwen_key_set: false,
        aws_profile: "",
        local_model_state: "missing",
        local_model_size: 0,
      };
    },
  );

  let localProgress: { bytes: number; total: number } | null = null;

  void listen<{ bytes: number; total: number }>(
    "model:download-progress",
    ({ payload }) => {
      localProgress = payload;
      config = { ...config, local_model_state: "downloading" };
      render();
    },
  );
  void listen("model:download-done", () => {
    localProgress = null;
    config = { ...config, local_model_state: "downloaded" };
    render();
  });
  void listen<{ message: string }>("model:download-error", ({ payload }) => {
    localProgress = null;
    config = { ...config, local_model_state: "error" };
    // The row already says the download failed; the reason only fits in the
    // console. Read here rather than dropped so `tsc` does not call it unused.
    console.error(payload.message);
    render();
  });

  render();

  for (const radio of radios) {
    radio.addEventListener("change", () => {
      if (!radio.checked) return;
      const newProvider = radio.value;
      void invoke("set_provider", { provider: newProvider })
        .then(() => {
          config = { ...config, provider: newProvider };
          render();
        })
        .catch((error) => {
          console.error(error);
          // The optimistic version was wrong: this window is created once and
          // never re-reads from settings, so painting a lie here persists for
          // the whole session. Repaint from the config the backend agrees with.
          render();
        });
    });
  }

  wireKeyField("kimi", "set_kimi_api_key", (set) => {
    config = { ...config, kimi_key_set: set };
    render();
  });
  wireKeyField("deepseek", "set_deepseek_api_key", (set) => {
    config = { ...config, deepseek_key_set: set };
    render();
  });
  wireKeyField("qwen", "set_qwen_api_key", (set) => {
    config = { ...config, qwen_key_set: set };
    render();
  });
  wireProfileField();

  // Unlike the keys, the profile name is not a secret, so the field shows it in
  // full and an empty box is an empty profile rather than a hidden one. Sent on
  // `change` (blur or Enter), not per keystroke — each save writes the file and
  // spends a warm-up request. Trimming here mirrors what Rust does before it
  // stores, so the two cannot disagree about what an empty field means.
  function wireProfileField() {
    const input = document.querySelector<HTMLInputElement>("#aws-profile");
    if (!input) return;

    input.addEventListener("change", () => {
      const profile = input.value.trim();
      void invoke("set_aws_profile", { profile })
        .then(() => {
          config = { ...config, aws_profile: profile };
          render();
        })
        .catch((error) => {
          console.error(error);
          render();
        });
    });
  }

  // On `change`, so the key is sent on blur or Enter rather than on every
  // keystroke — a key is pasted, not typed, and each save writes the file and
  // spends a warm-up request. An empty field means "unchanged", not "clear":
  // the stored key is never sent to this window, so an empty box is what a
  // configured key looks like here. Clearing is the button's job alone.
  function wireKeyField(
    prefix: "kimi" | "deepseek" | "qwen",
    command: "set_kimi_api_key" | "set_deepseek_api_key" | "set_qwen_api_key",
    applied: (set: boolean) => void,
  ) {
    const keyInput = document.querySelector<HTMLInputElement>(`#${prefix}-key`);
    const clearButton = document.querySelector<HTMLButtonElement>(
      `#${prefix}-clear`,
    );
    if (!keyInput || !clearButton) return;

    keyInput.addEventListener("change", () => {
      const key = keyInput.value.trim();
      if (!key) return;
      void invoke(command, { key })
        .then(() => {
          keyInput.value = "";
          applied(true);
        })
        .catch((error) => {
          console.error(error);
          // The optimistic version was wrong: repaint from what the backend
          // actually kept.
          render();
        });
    });

    clearButton.addEventListener("click", () => {
      void invoke(command, { key: "" })
        .then(() => {
          keyInput.value = "";
          applied(false);
        })
        .catch((error) => {
          console.error(error);
          render();
        });
    });
  }

  function render() {
    for (const radio of radios) radio.checked = radio.value === config.provider;

    renderProfile(config.aws_profile);

    // The key itself never reaches this window, so an empty field is what a
    // configured key looks like. A placeholder of masked dots stands in for the
    // hidden key so the box does not read as "nothing saved" while the state
    // beside it says "已配置".
    renderKey("kimi", config.kimi_key_set,
      "在 platform.moonshot.cn 获取。国际站的 key 需要设置 TONEMATE_KIMI_BASE_URL。");
    renderKey("deepseek", config.deepseek_key_set,
      "在 platform.deepseek.com 获取。");
    renderKey("qwen", config.qwen_key_set,
      "在百炼 / QwenCloud 获取。国际站的 key 需要设置 TONEMATE_QWEN_BASE_URL。");

    // Only the chosen provider's field is shown. Four boxes asking for four
    // different keys at once read as clutter, and a key typed into a provider
    // that is not selected is not what anyone means — the radio has already
    // picked the service, so its field is the only one worth showing. The `hidden`
    // attribute (not a class) also takes the field out of the tab order.
    for (const field of document.querySelectorAll<HTMLElement>(".key")) {
      field.hidden = field.dataset.provider !== config.provider;
    }

    // Says what will actually happen on the next translation, which is not
    // always what is ticked: a provider without a key falls back to Bedrock,
    // and saying so here is cheaper than letting the log be the only place it
    // shows.
    const active = document.querySelector<HTMLElement>("#active");
    if (active) {
      const name =
        config.provider === "kimi" ? "Kimi"
        : config.provider === "deepseek" ? "DeepSeek"
        : config.provider === "qwen" ? "Qwen"
        : config.provider === "local" ? "本地模型 (Hy-MT2)"
        : "AWS Bedrock";

      if (config.provider === "local") {
        const s = config.local_model_state;
        active.textContent =
          s === "downloaded" ? `当前使用: ${name}`
          : s === "downloading" ? `当前使用: ${name} —— 模型下载中`
          : s === "error" ? `当前使用: ${name} —— 模型下载失败，重新选择可重试`
          : `当前使用: ${name} —— 模型未下载`;
      } else {
        const keySet =
          config.provider === "kimi" ? config.kimi_key_set
          : config.provider === "deepseek" ? config.deepseek_key_set
          : config.provider === "qwen" ? config.qwen_key_set
          : true;
        active.textContent = keySet
          ? `当前使用: ${name}`
          : `当前使用: AWS Bedrock —— 已选 ${name} 但未填 API Key`;
      }
    }

    renderLocal();
  }

  /** Paints the local-model download state and progress bar. */
  function renderLocal() {
    const state = document.querySelector<HTMLElement>("#local-state");
    const bar = document.querySelector<HTMLElement>("#local-bar");
    const progress = document.querySelector<HTMLElement>("#local-progress");
    const hint = document.querySelector<HTMLElement>("#local-hint");
    if (!state || !bar || !progress || !hint) return;

    const s = config.local_model_state;
    state.textContent =
      s === "downloaded" ? "已下载"
      : s === "downloading" ? "下载中…"
      : s === "error" ? "下载失败"
      : "未下载";

    if (s === "downloading" && localProgress) {
      progress.hidden = false;
      bar.style.width = `${(localProgress.bytes / localProgress.total) * 100}%`;
      hint.textContent =
        `已下载 ${(localProgress.bytes / 1048576).toFixed(0)} MB / ${(localProgress.total / 1048576).toFixed(0)} MB`;
    } else {
      progress.hidden = true;
      hint.textContent =
        s === "downloaded" ? `模型已就绪（${(config.local_model_size / 1048576).toFixed(0)} MB），翻译在本机完成。`
        : s === "error" ? "下载失败，重新选择本地模型可重试。"
        : "选择后自动下载约 440 MB 的模型。";
    }
  }
}

/** Paints the AWS profile field and the Bedrock radio's note. The profile name
    is not a secret, so the input holds the real value rather than a masked
    placeholder; an empty field reads as "AWS 默认" because that is what Bedrock
    does with no profile. */
function renderProfile(awsProfile: string) {
  const input = document.querySelector<HTMLInputElement>("#aws-profile");
  if (input) input.value = awsProfile;

  const state = document.querySelector<HTMLElement>("#bedrock-state");
  if (state) state.textContent = awsProfile ? `profile: ${awsProfile}` : "AWS 默认";

  const hint = document.querySelector<HTMLElement>("#aws-hint");
  if (hint)
    hint.textContent = awsProfile
      ? "留空则回到 AWS 默认凭证链。"
      : "留空使用 AWS 默认凭证链(AWS_PROFILE 或 default profile)。";
}

/** Paints one provider's key row — placeholder, "已配置" state, and hint — from
    the boolean Rust sends instead of the key itself. */
function renderKey(
  prefix: "kimi" | "deepseek" | "qwen",
  keySet: boolean,
  emptyHint: string,
) {
  const keyInput = document.querySelector<HTMLInputElement>(`#${prefix}-key`);
  if (keyInput) keyInput.placeholder = keySet ? "••••••••" : "";

  const state = document.querySelector<HTMLElement>(`#${prefix}-state`);
  if (state) state.textContent = keySet ? "已配置" : "未配置";

  const hint = document.querySelector<HTMLElement>(`#${prefix}-hint`);
  if (hint)
    hint.textContent = keySet
      ? "已保存。输入新的 key 可替换,或点「清除」删除。"
      : emptyHint;
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
