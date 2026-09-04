import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

const appWindow = getCurrentWindow();

window.addEventListener("DOMContentLoaded", () => {
  const input = document.querySelector<HTMLInputElement>("#input")!;

  const focusInput = () => {
    input.focus();
    input.select();
  };

  focusInput();

  // The webview keeps its DOM across hide/show, so the input has to be
  // re-focused every time the window comes back.
  appWindow.onFocusChanged(({ payload: focused }) => {
    if (focused) focusInput();
  });

  input.addEventListener("keydown", async (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      const text = input.value.trim();
      // Fire and forget: the translation takes a second or two, and the bar
      // should disappear the moment Enter lands.
      if (text) void invoke("submit", { text }).catch(console.error);
      input.value = "";
      await appWindow.hide();
    } else if (e.key === "Escape") {
      e.preventDefault();
      await appWindow.hide();
    }
  });
});
