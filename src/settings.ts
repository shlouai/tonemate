import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

const appWindow = getCurrentWindow();

// Which swatch is ticked comes from Rust, which is where the accent is kept:
// asking is the only way to open on the colour the bar is actually wearing, from
// this launch or any earlier one. Read once, because this window is the only
// thing that ever writes it. An accent no swatch carries ticks none of them,
// which is the honest answer — the bar is showing something this window can no
// longer offer.
window.addEventListener("DOMContentLoaded", () => {
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
});

// Esc closes the window, which for this one means hiding it — Rust turns every
// close request into a hide. Asking to close rather than hiding directly is
// deliberate: it makes Esc, ⌘W and the red button one path instead of three, so
// there is a single place where what "closing the settings" means can change.
// Bound to the window rather than to a control: there is nothing here to focus
// yet, so a handler on an element would never fire.
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  e.preventDefault();
  void appWindow.close().catch(console.error);
});
