import { getCurrentWindow } from "@tauri-apps/api/window";

const appWindow = getCurrentWindow();

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
