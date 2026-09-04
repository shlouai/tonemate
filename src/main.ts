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
      if (text) await invoke("submit", { text });
      input.value = "";
      await appWindow.hide();
    } else if (e.key === "Escape") {
      e.preventDefault();
      await appWindow.hide();
    }
  });
});
