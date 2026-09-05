import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { LogicalSize, getCurrentWindow } from "@tauri-apps/api/window";

const appWindow = getCurrentWindow();

/** Fixed in tauri.conf.json; only the height follows the content. */
const WINDOW_WIDTH = 640;

window.addEventListener("DOMContentLoaded", () => {
  const bar = document.querySelector<HTMLDivElement>(".bar")!;
  const input = document.querySelector<HTMLInputElement>("#input")!;
  const output = document.querySelector<HTMLDivElement>("#output")!;

  // The window is chromeless and transparent, so anything taller than it just
  // gets clipped — the window has to be told to grow with the result box.
  // Coalesced into a frame so a burst of streamed fragments costs one resize
  // instead of one per fragment.
  let resizeQueued = false;
  const syncWindowHeight = () => {
    if (resizeQueued) return;
    resizeQueued = true;
    requestAnimationFrame(() => {
      resizeQueued = false;
      const height = Math.ceil(bar.getBoundingClientRect().height);
      void appWindow
        .setSize(new LogicalSize(WINDOW_WIDTH, height))
        .catch(console.error);
    });
  };

  syncWindowHeight();

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

  const resetOutput = () => {
    output.textContent = "";
    output.classList.remove("error");
    output.hidden = true;
    syncWindowHeight();
  };

  void listen("translate:start", () => {
    output.textContent = "";
    output.classList.remove("error");
    output.hidden = false;
    syncWindowHeight();
  });

  void listen<string>("translate:delta", ({ payload }) => {
    output.textContent += payload;
    // Once the box hits its max height it scrolls; follow the tail.
    output.scrollTop = output.scrollHeight;
    syncWindowHeight();
  });

  // Replaces whatever streamed in before the failure: a truncated translation
  // is worse than none, because there's no way to tell it apart from a whole one.
  void listen<string>("translate:error", ({ payload }) => {
    output.classList.add("error");
    output.textContent = `⚠ ${payload}`;
    output.hidden = false;
    syncWindowHeight();
  });

  input.addEventListener("keydown", async (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      const text = input.value.trim();
      // Fire and forget: the bar stays up and fills in from the events above
      // as the translation streams back.
      if (text) void invoke("submit", { text }).catch(console.error);
      input.value = "";
    } else if (e.key === "Escape") {
      e.preventDefault();
      // Clear on the way out so the next summon is a bare input bar.
      resetOutput();
      await appWindow.hide();
    }
  });
});
