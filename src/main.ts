import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { LogicalSize, getCurrentWindow } from "@tauri-apps/api/window";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

const appWindow = getCurrentWindow();

/** Fixed in tauri.conf.json; only the height follows the content. */
const WINDOW_WIDTH = 640;

/** Payload of `translate:tone`. `text` is the row's full text, not a delta. */
type Tone = { index: number; label: string; text: string };

window.addEventListener("DOMContentLoaded", () => {
  const bar = document.querySelector<HTMLDivElement>(".bar")!;
  const input = document.querySelector<HTMLInputElement>("#input")!;
  const output = document.querySelector<HTMLDivElement>("#output")!;
  const loading = document.querySelector<HTMLDivElement>("#loading")!;
  const grip = document.querySelector<HTMLDivElement>("#grip")!;
  const toast = document.querySelector<HTMLDivElement>("#toast")!;

  // The window is undecorated, so there is no title bar to drag it by — the grip
  // is it. Preventing the default is what keeps the caret in the input: without
  // it, pressing a plain div moves focus off the input, and you'd have to click
  // back into the bar before typing.
  grip.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    void appWindow.startDragging().catch(console.error);
  });

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

  // The message is written into the element rather than only faded in, because
  // `role="status"` announces the change in text and not the change in opacity —
  // which is also why it is emptied a beat after the fade rather than with it.
  let toastTimer: number | undefined;
  const flashToast = (message: string) => {
    clearTimeout(toastTimer);
    toast.textContent = message;
    toast.classList.add("shown");
    toastTimer = window.setTimeout(() => {
      toast.classList.remove("shown");
      toastTimer = window.setTimeout(() => {
        toast.textContent = "";
      }, 200);
    }, 1400);
  };

  const clearToast = () => {
    clearTimeout(toastTimer);
    toast.classList.remove("shown");
    toast.textContent = "";
  };

  // Rows are keyed by tone index because `translate:tone` carries a row's whole
  // text rather than a delta: an update is a write, not an append, so a repeated
  // event cannot corrupt a row.
  const rows = new Map<number, HTMLSpanElement>();

  const clearRows = () => {
    rows.clear();
    output.replaceChildren();
    output.classList.remove("error");
    // The confirmation referred to a row that is about to stop existing.
    clearToast();
  };

  /** Builds a row, returning the element its text goes in. */
  const appendRow = (label: string): HTMLSpanElement => {
    const row = document.createElement("div");
    // A row the model didn't label has no label element at all, so its text can
    // take both grid columns instead of sitting in the narrow one.
    row.className = label ? "tone" : "tone unlabelled";
    if (label) {
      const labelEl = document.createElement("span");
      labelEl.className = "label";
      labelEl.textContent = label;
      row.append(labelEl);
    }
    const text = document.createElement("span");
    text.className = "text";
    row.append(text);
    output.append(row);
    return text;
  };

  const resetOutput = () => {
    clearRows();
    output.hidden = true;
    loading.hidden = true;
    syncWindowHeight();
  };

  // Delegated to the container because the rows themselves are thrown away and
  // rebuilt on every translation. Mouse-only by design: making a row focusable
  // would give the caret somewhere to go other than the input, and keeping it in
  // the input is the whole shape of this window.
  output.addEventListener("click", (e) => {
    // Nothing in an error message is worth putting on the clipboard.
    if (output.classList.contains("error")) return;
    const row = (e.target as HTMLElement).closest(".tone");
    if (!row) return;
    // A press that ends a selection is not a click on the row: the text is
    // selectable by hand, and copying the whole row over a hand-picked fragment
    // would throw away the more deliberate of the two.
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed) return;
    // The label names the row, it isn't part of it — what gets pasted is the
    // rendering alone.
    const text = row.querySelector(".text")?.textContent;
    if (!text) return;
    // Through the native pasteboard, not `navigator.clipboard`: WebKit rejects
    // that with NotAllowedError unless it recognises a user gesture, and a click
    // on a plain div doesn't reliably qualify.
    void writeText(text).then(
      () => flashToast("已拷贝至剪切板"),
      (err: unknown) => {
        console.error(err);
        flashToast("拷贝失败");
      },
    );
    // Pressing a div takes the caret out of the input. Put it back — but without
    // selecting, unlike a fresh summon, so a half-typed next query survives.
    input.focus();
  });

  // The box opens on the placeholder rather than on emptiness: there's most of a
  // second between the request going out and the first fragment coming back, and
  // a blank box that size reads as a bug rather than as work in progress.
  void listen("translate:start", () => {
    clearRows();
    output.hidden = true;
    loading.hidden = false;
    syncWindowHeight();
  });

  void listen<Tone>("translate:tone", ({ payload }) => {
    loading.hidden = true;
    output.hidden = false;
    // Appending is enough to keep the rows in order: the model writes them in
    // order and the events arrive in the order they were emitted.
    let text = rows.get(payload.index);
    if (!text) {
      text = appendRow(payload.label);
      rows.set(payload.index, text);
    }
    text.textContent = payload.text;
    // Once the box hits its max height it scrolls; follow the tail.
    output.scrollTop = output.scrollHeight;
    syncWindowHeight();
  });

  // Replaces whatever streamed in before the failure: a half-finished set of
  // renderings is worse than none, because there's no way to tell it apart from
  // a complete one.
  void listen<string>("translate:error", ({ payload }) => {
    loading.hidden = true;
    clearRows();
    output.classList.add("error");
    // Through a row rather than straight onto #output: that element is a grid
    // now, so bare text would become an anonymous grid item and get squeezed
    // into the label column.
    appendRow("").textContent = `⚠ ${payload}`;
    output.hidden = false;
    syncWindowHeight();
  });

  // Only does anything when the model answered with nothing at all — any other
  // response retired the placeholder on its first rendering. Without this the
  // dots would keep pulsing until Esc, promising a result that is never coming;
  // collapsing back to a bare bar at least says so. Once the set is complete,
  // scroll back to the top so the finished set is read from its anchor (the first
  // row is always the most literal).
  void listen("translate:done", () => {
    loading.hidden = true;
    output.scrollTop = 0;
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
