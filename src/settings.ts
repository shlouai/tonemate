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
