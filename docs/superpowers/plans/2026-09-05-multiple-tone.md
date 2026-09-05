# Multiple Tones Per Translation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Answer one input with 3-5 labelled translations that differ in tone, register, and directness, streamed into a read-only list under the input bar.

**Architecture:** One streamed Bedrock call whose system prompt asks for `LABEL<TAB>TRANSLATION`, one line per rendering. A new pure-Rust parser (`tones.rs`) turns the fragment stream into per-row updates; `lib.rs` emits each update as a `translate:tone` event; `main.ts` upserts rows into a CSS grid so every translation aligns at the same left edge.

**Tech Stack:** Rust (Tauri 2, aws-sdk-bedrockruntime, serde), TypeScript (vite, `@tauri-apps/api`), plain CSS.

**Spec:** `docs/superpowers/specs/2026-09-05-multiple-tone-design.md`

## Global Constraints

- Tone labels are always Chinese, 2-4 characters, whichever direction the translation runs.
- The model chooses how many renderings to give, between 3 and 5. Never pad to a fixed count.
- The label/translation separator is a single tab (`\t`); rows are separated by `\n`. A translation never contains a line break.
- `Tone.text` is a row's **full text so far**, not a delta. Consumers overwrite by `index`.
- Malformed model output degrades to displaying text, never to an error.
- Run all `cargo` commands from `src-tauri/`; run all `pnpm` commands from the repo root.
- Commit messages follow the existing repo style: `type: subject` in the imperative, a body explaining *why*, then `Rejected: <option> | <reason>` lines for alternatives considered, `Confidence: <level>`, `Not-tested: <what and why>`, and finally `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- This environment cannot send keystrokes to the GUI. Rust behaviour is verified with `cargo test` and `cargo run --example translate`; the window itself is checked by the human.

---

### Task 1: The line parser

The one piece of this feature testable without AWS credentials or a window. Build it first and completely, so later tasks are only wiring.

**Files:**
- Create: `src-tauri/src/tones.rs` (implementation and its `#[cfg(test)] mod tests`)
- Modify: `src-tauri/src/lib.rs:1` (add `pub mod tones;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Tone { pub index: usize, pub label: String, pub text: String }`, deriving `Debug, Clone, PartialEq, serde::Serialize`. `Clone` and `Serialize` are both required by Tauri's `emit`; `Debug`/`PartialEq` are for `assert_eq!`.
  - `pub struct Parser` with `Default`, `pub fn push(&mut self, fragment: &str) -> Vec<Tone>`, and `pub fn finish(&mut self) -> Vec<Tone>`.

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/tones.rs` containing *only* the test module below plus the two declarations it needs to compile against. Write the tests first; the bodies of `push`/`finish` come in step 3.

```rust
//! Turns the model's line-delimited output into row updates.

use serde::Serialize;

/// One rendering of the input. `text` is the row's full text so far rather than
/// a delta: consumers overwrite by `index`, so a repeated or reordered update
/// cannot corrupt a row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tone {
    pub index: usize,
    pub label: String,
    pub text: String,
}

#[derive(Default)]
pub struct Parser {}

impl Parser {
    pub fn push(&mut self, _fragment: &str) -> Vec<Tone> {
        todo!()
    }

    pub fn finish(&mut self) -> Vec<Tone> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn tone(index: usize, label: &str, text: &str) -> Tone {
        Tone {
            index,
            label: label.to_string(),
            text: text.to_string(),
        }
    }

    /// Every update the parser emitted, in order.
    fn feed(fragments: &[&str]) -> Vec<Tone> {
        let mut parser = Parser::default();
        let mut updates = Vec::new();
        for fragment in fragments {
            updates.extend(parser.push(fragment));
        }
        updates.extend(parser.finish());
        updates
    }

    /// What a consumer would be showing once the stream ends: the updates
    /// applied as upserts, in index order.
    fn rows(fragments: &[&str]) -> Vec<Tone> {
        let mut rows: BTreeMap<usize, Tone> = BTreeMap::new();
        for update in feed(fragments) {
            rows.insert(update.index, update);
        }
        rows.into_values().collect()
    }

    #[test]
    fn splits_rows_on_newlines_and_labels_on_tabs() {
        assert_eq!(
            rows(&["直译\tI can't come tomorrow.\n正式\tI'm afraid I can't.\n"]),
            vec![
                tone(0, "直译", "I can't come tomorrow."),
                tone(1, "正式", "I'm afraid I can't."),
            ]
        );
    }

    /// Where this parser is most likely to break: a tab or a newline landing on
    /// a fragment boundary must not change the outcome.
    #[test]
    fn feeding_one_character_at_a_time_gives_the_same_rows() {
        let whole = "直译\tI can't come tomorrow.\n正式\tI'm afraid I can't.";
        let chars: Vec<String> = whole.chars().map(|c| c.to_string()).collect();
        let fragments: Vec<&str> = chars.iter().map(String::as_str).collect();

        assert_eq!(rows(&fragments), rows(&[whole]));
    }

    /// The property that makes the result stream: the label is fixed the moment
    /// its tab lands, so the row can fill in afterwards.
    #[test]
    fn the_label_is_emitted_before_its_translation_arrives() {
        let mut parser = Parser::default();

        assert_eq!(parser.push("直"), vec![]);
        assert_eq!(parser.push("译\t"), vec![tone(0, "直译", "")]);
        assert_eq!(parser.push("I can"), vec![tone(0, "直译", "I can")]);
    }

    #[test]
    fn padding_around_the_label_and_translation_is_trimmed() {
        assert_eq!(
            rows(&["  直译 \t  I can't come tomorrow.  \n"]),
            vec![tone(0, "直译", "I can't come tomorrow.")]
        );
    }

    #[test]
    fn blank_lines_do_not_consume_an_index() {
        assert_eq!(
            rows(&["直译\tA\n\n   \n正式\tB\n"]),
            vec![tone(0, "直译", "A"), tone(1, "正式", "B")]
        );
    }

    #[test]
    fn the_last_row_survives_a_missing_trailing_newline() {
        assert_eq!(
            rows(&["直译\tA\n正式\tB"]),
            vec![tone(0, "直译", "A"), tone(1, "正式", "B")]
        );
    }

    #[test]
    fn a_row_without_a_tab_becomes_unlabelled() {
        assert_eq!(
            rows(&["直译\tA\nJust a sentence.\n"]),
            vec![tone(0, "直译", "A"), tone(1, "", "Just a sentence.")]
        );
    }

    #[test]
    fn output_ignoring_the_format_entirely_is_still_shown() {
        assert_eq!(
            rows(&["Just a sentence."]),
            vec![tone(0, "", "Just a sentence.")]
        );
    }

    #[test]
    fn only_the_first_tab_splits_a_row() {
        assert_eq!(rows(&["直译\tA\tB\n"]), vec![tone(0, "直译", "A\tB")]);
    }

    #[test]
    fn carriage_returns_are_stripped() {
        assert_eq!(
            rows(&["直译\tA\r\n正式\tB\r\n"]),
            vec![tone(0, "直译", "A"), tone(1, "正式", "B")]
        );
    }

    #[test]
    fn whitespace_only_output_yields_no_rows() {
        assert_eq!(rows(&["\n  \n\n"]), vec![]);
    }
}
```

Then add the module declaration at the top of `src-tauri/src/lib.rs`, above the existing `pub mod bedrock;`:

```rust
pub mod bedrock;
pub mod tones;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd src-tauri && cargo test --lib tones`
Expected: the tests compile, then every one of them fails by panicking on `not yet implemented` from the `todo!()` bodies.

- [ ] **Step 3: Write the implementation**

Replace the placeholder `Parser` in `src-tauri/src/tones.rs` with the real one. Keep the `Tone` struct and the test module exactly as written in step 1.

```rust
/// The stream arrives in arbitrary fragments — a tab or a newline can land on
/// any boundary — so the split has to be resumable rather than a one-shot
/// `lines()` over a finished string.
#[derive(Default)]
pub struct Parser {
    /// The row still arriving, up to but not including its newline.
    pending: String,
    /// Index the row in `pending` will carry. Blank lines never advance it, so
    /// stray spacing in the model's output doesn't leave holes.
    next_index: usize,
}

impl Parser {
    /// Rows this fragment changed. A fragment carrying a newline touches two:
    /// the row it closed and the row it opened.
    pub fn push(&mut self, fragment: &str) -> Vec<Tone> {
        let mut updates = Vec::new();
        self.pending.push_str(fragment);

        while let Some(newline) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=newline).collect();
            if let Some(tone) = self.row(&line) {
                updates.push(tone);
                self.next_index += 1;
            }
        }

        // Until the tab arrives there is no way to tell a label from the start
        // of a translation, so a row with no tab yet has nothing to show.
        if self.pending.contains('\t') {
            updates.extend(self.row(&self.pending));
        }

        updates
    }

    /// Closes the row still buffered, since the model usually omits the
    /// trailing newline. Without this the last rendering would be dropped.
    pub fn finish(&mut self) -> Vec<Tone> {
        let line = std::mem::take(&mut self.pending);
        match self.row(&line) {
            Some(tone) => {
                self.next_index += 1;
                vec![tone]
            }
            None => Vec::new(),
        }
    }

    /// A line — complete or still arriving — as a row. `None` for a blank line,
    /// which must not consume an index.
    ///
    /// The line is deliberately not trimmed as a whole before the split: a row
    /// that has just received its tab ends in one, and trimming would take that
    /// tab away and make the label look like an unlabelled translation.
    fn row(&self, line: &str) -> Option<Tone> {
        let (label, text) = match line.split_once('\t') {
            // A translation may legitimately contain a tab; only the first one
            // separates.
            Some((label, text)) => (label.trim(), text.trim()),
            None => ("", line.trim()),
        };

        if label.is_empty() && text.is_empty() {
            return None;
        }

        Some(Tone {
            index: self.next_index,
            label: label.to_string(),
            text: text.to_string(),
        })
    }
}
```

Note on `drain(..=newline)`: `find` returns a byte index and `\n` is one byte at a char boundary, so the inclusive byte range is always valid even with multi-byte labels.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd src-tauri && cargo test --lib tones`
Expected: PASS, 11 tests.

Then check nothing else broke and there are no warnings worth keeping:

Run: `cd src-tauri && cargo test && cargo clippy --lib --tests`
Expected: all tests pass; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tones.rs src-tauri/src/lib.rs
git commit -F - <<'EOF'
feat: parse the model's tab-delimited renderings into rows

Groundwork for answering one input with several differently-toned translations:
the response stops being one blob of text and becomes one `LABEL<TAB>TRANSLATION`
line per rendering, which something has to split.

The split is resumable rather than a pass over a finished string, because the
whole point is to keep streaming: fragments arrive at arbitrary boundaries, so a
tab or a newline can land anywhere, and a row's label has to be pinned the
instant its tab shows up if the translation is going to fill in afterwards. Each
update carries the row's full text rather than a delta, so consumers overwrite by
index and hold no accumulation state of their own.

Malformed output degrades to showing text: a line with no tab becomes an
unlabelled row instead of an error, since a translation the reader can use beats
a parser complaint.

Rejected: splitting the response after it completes | gives up streaming, the property that makes the bar feel immediate
Rejected: emitting deltas per row | makes both sides carry accumulation state to save resending one sentence
Confidence: high
Not-tested: nothing — this is the one piece of the feature that runs without AWS credentials, and its tests cover tab/newline fragment boundaries, blank lines, a missing trailing newline, and format the model got wrong

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 2: Ask the model for several tones

Independently checkable: after this task the terminal shows tab-delimited labelled lines, even though the window still renders them as one text blob.

**Files:**
- Modify: `src-tauri/src/bedrock.rs:34-37` (`SYSTEM_PROMPT`), `src-tauri/src/bedrock.rs:152` (`max_tokens`)

**Interfaces:**
- Consumes: nothing from Task 1 — `bedrock.rs` keeps its current job of streaming text fragments, and no signature changes.
- Produces: model output shaped as `LABEL<TAB>TRANSLATION` lines, which Task 1's parser and Task 3's wiring both assume.

- [ ] **Step 1: Replace the system prompt**

In `src-tauri/src/bedrock.rs`, replace the `SYSTEM_PROMPT` constant and its doc comment (currently lines 31-37) with:

```rust
/// The direction is the model's call, not a character-class check in Rust: it
/// already reads the text, and input that mixes scripts — a Chinese sentence
/// carrying one English word — would fool any threshold we picked.
///
/// The tab-delimited line format is what lets the answer stream: each label is
/// fixed the moment its tab arrives, so a rendering fills in character by
/// character instead of appearing all at once at the end of the response.
const SYSTEM_PROMPT: &str = "You are a translation engine. If the user's text is English, translate \
     it into natural, idiomatic Chinese; otherwise translate it into natural, idiomatic English.\n\
     Work out what the writer is doing first: what they want from the reader, how they stand in \
     relation to that reader, and how blunt the original was. Then give 3 to 5 renderings that \
     differ in tone, register, and directness, each one the right choice in some concrete \
     situation. If only three are meaningfully different, give three — never pad the list with \
     near-duplicates.\n\
     The first line is the most faithful, most neutral rendering. Each later line sits further \
     from it in tone.\n\
     Output one rendering per line: a label in Chinese of 2 to 4 characters, then a single tab \
     character, then the translation. No numbering, no blank lines, no markdown, no quotes, no \
     explanation, and never a line break inside a translation.";
```

- [ ] **Step 2: Raise the token budget**

In `src-tauri/src/bedrock.rs`, `translate` currently calls `converse(text, 2048, on_delta)`. The same input now produces four or five renderings, so the old budget truncates inputs that used to fit. Change the call and document why:

```rust
pub async fn translate(text: &str, on_delta: impl FnMut(&str)) -> Result<String, String> {
    // Four or five renderings of the same input, so roughly five times the
    // budget one translation needed.
    let (translated, stop_reason) = converse(text, 4096, on_delta).await?;
```

Leave `warm()` alone: it still asks for one token, and truncating the warm-up after one token is the point.

- [ ] **Step 3: Check the real model against the format**

Run: `cd src-tauri && cargo run --example translate -- "我明天不能来了"`

Expected: after `[tonemate] out: `, three to five lines, each a short Chinese label, then a tab, then an English translation, the first line the plainest. Something like:

```
直译	I can't come tomorrow.
正式	I'm afraid I won't be able to make it tomorrow.
委婉	Something's come up — would another day work?
干脆	Not coming tomorrow.
```

Also confirm the direction still flips, and that a heavier input doesn't blow past the budget:

Run: `cd src-tauri && cargo run --example translate -- "Sorry, I'm running a few minutes late."`
Expected: labelled lines with Chinese translations.

Check the reported `first word in ...` / `total ...` numbers. Inferring intent and writing five renderings is heavier than one literal translation, so if the total time is much worse than the ~1.8s the README quotes, note the figure — `TONEMATE_EFFORT` is the knob, and whether `low` still suffices is a real question this run answers. Record what you saw in the commit's `Not-tested:`/body rather than silently accepting a regression.

If the model ignores the tab and uses `：` or `-` instead, tighten the prompt's last paragraph rather than teaching the parser a second separator: one format is the contract.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/bedrock.rs
git commit -F - <<'EOF'
feat: ask for 3-5 renderings that differ in tone

One translation hides the choice that actually matters. The same sentence can be
blunt or deferential, and which one is right depends on who is going to read it,
so the bar was making the decision silently and the only way to see the
alternative was to re-ask with "more polite" bolted on.

The prompt now infers what the writer is doing — what they want from the reader,
how they stand to them, how blunt the original was — and answers with 3 to 5
renderings spread across tone, register, and directness, ordered outwards from
the most faithful one. The count is the model's to choose: five renderings that
read alike are worse than three that don't.

Each line is `LABEL<TAB>TRANSLATION`. A tab keeps the format free of escaping
and, unlike a JSON document, is readable before it is complete, so the answer can
still stream. The budget goes to 4096 because the same input now produces five
answers where it used to produce one.

Rejected: a fixed set of tone labels | the useful tones depend on the input, and a constant list forces padding when only three apply
Rejected: labels in the target language | the label is there for the person who typed the input, not for the reader of the translation
Confidence: medium
Not-tested: the GUI path, which this machine cannot drive from a script; verified through `cargo run --example translate` in both directions, which returned labelled tab-delimited lines ordered from literal outwards

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 3: Emit one event per row

**Files:**
- Modify: `src-tauri/src/lib.rs:7-40` (`submit`)
- Modify: `src-tauri/examples/translate.rs:28` and `:31-38` (stdout prefix, and show the parsed rows)

**Interfaces:**
- Consumes: `tones::Parser::default()`, `Parser::push(&str) -> Vec<Tone>`, `Parser::finish() -> Vec<Tone>`, `Tone { index, label, text }` from Task 1.
- Produces: the `translate:tone` event carrying `{ index: number, label: string, text: string }`, which Task 4 listens for. `translate:delta` is gone. `translate:start`, `translate:done`, and `translate:error` keep their current payloads and meaning.

- [ ] **Step 1: Drive the parser from `submit`**

In `src-tauri/src/lib.rs`, replace the body of `submit` (and update its doc comment) with:

```rust
/// Translate what was typed, streaming the renderings to both the terminal and
/// the result box under the input. The frontend doesn't await this — it just
/// listens for the events below, so the bar stays responsive while the model
/// answers.
#[tauri::command]
async fn submit(window: WebviewWindow, text: String) {
    println!("[tonemate] in : {text}");
    // The renderings arrive on lines of their own, so the label gets its own
    // line too rather than sitting in front of the first one.
    println!("[tonemate] out:");
    // stdout is line-buffered, so each fragment needs an explicit flush to
    // actually appear as it arrives rather than all at once at the newline.
    let _ = std::io::stdout().flush();

    let _ = window.emit("translate:start", ());

    let mut parser = tones::Parser::default();
    let result = bedrock::translate(&text, |fragment| {
        print!("{fragment}");
        let _ = std::io::stdout().flush();
        for tone in parser.push(fragment) {
            let _ = window.emit("translate:tone", tone);
        }
    })
    .await;

    println!();
    match result {
        // The frontend needs the end of the stream, not just its fragments: a
        // response that streams nothing would otherwise leave the loading
        // placeholder up forever. `finish` comes first because the model
        // usually omits the trailing newline, so the last rendering is still
        // sitting in the parser at this point.
        Ok(_) => {
            for tone in parser.finish() {
                let _ = window.emit("translate:tone", tone);
            }
            let _ = window.emit("translate:done", ());
        }
        Err(err) => {
            eprintln!("[tonemate] translate failed: {err}");
            let _ = window.emit("translate:error", err);
        }
    }
}
```

`parser` is used again after the `.await` even though the closure borrowed it mutably; that is fine because the future holding the closure is dropped at the end of the `let result = ...;` statement, which ends the borrow.

- [ ] **Step 2: Show the parsed rows in the example**

`examples/translate.rs` is how the Rust side gets verified without a window, so it should exercise the parser too — that is the only way to see whether the prompt and the parser agree about the real model's output. In `src-tauri/examples/translate.rs`, add the import at the top:

```rust
use std::collections::BTreeMap;
use std::io::Write;
use std::time::Instant;
```

Then replace the print prefix and the `translate` call (currently lines 28-39) with:

```rust
        println!("[tonemate] out:");
        let _ = std::io::stdout().flush();

        let started = Instant::now();
        let mut first_word = None;
        let mut parser = tonemate_lib::tones::Parser::default();
        // Keyed by index because updates carry a row's full text, so a later
        // update for the same row replaces the earlier one.
        let mut rows: BTreeMap<usize, String> = BTreeMap::new();
        let result = tonemate_lib::bedrock::translate(&text, |fragment| {
            first_word.get_or_insert_with(|| started.elapsed());
            print!("{fragment}");
            let _ = std::io::stdout().flush();
            for tone in parser.push(fragment) {
                rows.insert(tone.index, format!("[{}] {}", tone.label, tone.text));
            }
        })
        .await;
        println!();
        for tone in parser.finish() {
            rows.insert(tone.index, format!("[{}] {}", tone.label, tone.text));
        }
```

And extend the success arm (currently lines 42-46) so the parse is visible:

```rust
        match result {
            Ok(_) => {
                println!("[tonemate] parsed {} tones:", rows.len());
                for row in rows.values() {
                    println!("  {row}");
                }
                println!(
                    "[tonemate] first word in {:?}, total {:?}",
                    first_word.unwrap_or_default(),
                    started.elapsed()
                );
            }
            Err(err) => {
                eprintln!("[tonemate] translate failed: {err}");
                std::process::exit(1);
            }
        }
```

Also update the example's module doc comment (line 5-6) to match what it now prints:

```rust
//! Prints the raw stream, then the parsed renderings, then time-to-first-word
//! and total time. Runs the same warm-up the app does at startup so the numbers
//! reflect a warm client rather than a cold one.
```

- [ ] **Step 3: Verify the wiring**

Run: `cd src-tauri && cargo test && cargo clippy --lib --examples --tests`
Expected: Task 1's 11 tests still pass; clippy clean. In particular there must be no unused-variable warning for a leftover `translate:delta` path.

Run: `cd src-tauri && cargo run --example translate -- "我明天不能来了"`
Expected: the raw stream as in Task 2, then a block like

```
[tonemate] parsed 4 tones:
  [直译] I can't come tomorrow.
  [正式] I'm afraid I won't be able to make it tomorrow.
  [委婉] Something's come up — would another day work?
  [干脆] Not coming tomorrow.
[tonemate] first word in 1.1s, total 2.4s
```

Every row must have a non-empty label. An empty `[]` means the model broke format and the parser fell back — go fix the prompt (Task 2, step 3) rather than accepting it.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs src-tauri/examples/translate.rs
git commit -F - <<'EOF'
feat: emit each rendering as its own event

The frontend was handed raw fragments and appended them to one box, which cannot
work now that the response is several renderings: the box would show the tab
characters and the labels inline. `translate:tone` replaces `translate:delta` and
carries `{ index, label, text }`, where `text` is that row's full text so far —
so the receiver overwrites by index instead of appending, and holds no
accumulation state that a repeated or reordered event could corrupt.

`finish` runs before `translate:done` because the model usually omits the
trailing newline, which leaves the last rendering sitting in the parser when the
stream ends.

The terminal keeps getting the raw stream, with `out:` moved onto a line of its
own so the renderings below it line up instead of the first one being indented
past a prefix. The example now also prints what the parser made of the response,
because that is the only way to check the prompt and the parser agree about the
real model's output without a window to look at.

Rejected: parsing in the webview | the tab-delimited format is the Rust side's contract with the model, and TypeScript would need its own resumable splitter to keep streaming
Confidence: high
Not-tested: the events reaching the window, which needs the GUI this machine cannot drive; verified through `cargo run --example translate`, where the parser recovered every labelled rendering from the live stream

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 4: Render the rows

**Files:**
- Modify: `src/main.ts:59-104` (row upsert replacing the append-text listeners)
- Modify: `src/styles.css:37-68` (the `#output` block becomes a grid; row and label styling)
- Modify: `README.md:3-6` and `README.md:35-45` (behaviour and sample terminal output)
- No change needed to `index.html`: `#output` stays as it is and the rows inside it are built by JS.

**Interfaces:**
- Consumes: the `translate:tone` event from Task 3, payload `{ index: number, label: string, text: string }`; plus the unchanged `translate:start`, `translate:done`, `translate:error`.
- Produces: nothing further depends on this.

- [ ] **Step 1: Upsert rows in `main.ts`**

In `src/main.ts`, add the payload type just below the `WINDOW_WIDTH` constant:

```ts
/** Payload of `translate:tone`. `text` is the row's full text, not a delta. */
type Tone = { index: number; label: string; text: string };
```

Then replace `resetOutput` and the four `listen` calls (currently lines 59-104) with:

```ts
  // Rows are keyed by tone index because `translate:tone` carries a row's whole
  // text rather than a delta: an update is a write, not an append, so a repeated
  // event cannot corrupt a row.
  const rows = new Map<number, HTMLSpanElement>();

  const clearRows = () => {
    rows.clear();
    output.replaceChildren();
    output.classList.remove("error");
  };

  /** Builds a row, returning the element its text goes in. */
  const appendRow = (label: string): HTMLSpanElement => {
    const row = document.createElement("div");
    // A row the model didn't label has no label element at all, so its text can
    // take both grid columns instead of sitting in the narrow one.
    row.className = label ? "tone" : "tone unlabeled";
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
  // collapsing back to a bare bar at least says so.
  void listen("translate:done", () => {
    loading.hidden = true;
    syncWindowHeight();
  });
```

- [ ] **Step 2: Make `#output` a grid in `styles.css`**

In `src/styles.css`, replace the block from the `#loading, #output` comment through `#output.error` (currently lines 37-68) with:

```css
/* The result rows and the placeholder that stands in for them occupy the same
   slot under the input, so they share its chrome: swapping one for the other
   must not move anything that was already on screen. */
#loading,
#output {
  margin: 6px 10px 2px;
  padding-top: 8px;
  border-top: 1px solid rgba(255, 255, 255, 0.12);
}

/* The grid is on the container rather than on each row, so the label column is
   sized by the widest label in the whole set and every translation starts at the
   same left edge. Rows hand their two spans straight to these columns via
   `display: contents`. */
#output {
  display: grid;
  grid-template-columns: max-content 1fr;
  column-gap: 10px;
  row-gap: 6px;
  /* Caps how far the window can grow; longer results scroll instead. */
  max-height: 240px;
  overflow-y: auto;
  /* Lets the text be selected and copied even though these are divs rather than
     real inputs. */
  user-select: text;
}

#loading[hidden],
#output[hidden] {
  display: none;
}

.tone {
  display: contents;
}

/* Held against the first line of its translation rather than centred against a
   wrapped block: the label names the row, and the row starts at the top. The
   padding is the nudge that lines 13px text up with 15px/1.45 text. */
.label {
  align-self: start;
  padding-top: 2px;
  font-size: 13px;
  white-space: nowrap;
  color: rgba(242, 242, 247, 0.45);
  user-select: none;
}

.text {
  color: #f2f2f7;
  font-size: 15px;
  line-height: 1.45;
  /* Keeps the model's own spacing. */
  white-space: pre-wrap;
  overflow-wrap: break-word;
}

/* An unlabelled row — and the error message, which is one — has no label
   element, so its text takes both columns. */
.tone.unlabeled .text {
  grid-column: 1 / -1;
}

#output.error .text {
  color: #ff6b6b;
}
```

The `#loading` rule that follows keeps its `height: 1.45em` comment reference to `#output` text; that height now matches `.text`, which carries the same 15px/1.45. Update that comment's wording from `#output` to `.text`:

```css
/* Exactly one line of result text tall (1.45em of the same 15px `.text` uses),
   so the first rendering replaces the dots in place instead of resizing the
   window. */
```

- [ ] **Step 3: Type-check and build**

Run: `pnpm build`
Expected: `tsc` reports no errors and vite writes `dist/`. A `Property 'index' does not exist` style error means the `Tone` type and the `listen<Tone>` call disagree — fix the type, not the call site.

- [ ] **Step 4: Update the README**

Two places describe the old single-result behaviour. Replace the opening paragraph (lines 3-6):

```markdown
A Spotlight-style floating input bar: hit `Cmd+Shift+Space` anywhere, type text in
any language, press Enter, and several translations stream into a box under the
input — the same sentence rendered in 3 to 5 different tones, each labelled, so
you can pick the one that fits who is reading it. The direction picks itself —
English in gets Chinese back, anything else gets English. Translation runs through
Claude on Amazon Bedrock.
```

And the result-box paragraph with its sample output (lines 35-45):

```markdown
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
直译	I can't come tomorrow.
正式	I'm afraid I won't be able to make it tomorrow.
委婉	Something's come up — would another day work?
干脆	Not coming tomorrow.
```
```

Replace the sample renderings with what your Task 3 run actually produced, so the README documents real output rather than invented output.

- [ ] **Step 5: Check it in the window**

This environment cannot send keystrokes to the GUI, so this step is the human's. Run `pnpm tauri dev`, wait for `[tonemate] hotkey registered`, then ask them to press `Cmd+Shift+Space`, type `我明天不能来了`, and press Enter. What to confirm:

- Labels appear before their translations fill in, and every translation starts at the same left edge
- The window grows to fit all the rows and stops growing at the point the rows start scrolling
- `Esc` hides the bar and the next summon is a bare input with no leftover rows
- An error still shows in red as a single full-width line: force one with
  `TONEMATE_MODEL=nope pnpm tauri dev`, which makes the warm-up and the
  translation both fail with Bedrock's "provided model identifier is invalid"

Do not claim this task is done on the strength of `pnpm build`. If the human hasn't looked at the window yet, say so.

- [ ] **Step 6: Commit**

```bash
git add src/main.ts src/styles.css README.md
git commit -F - <<'EOF'
feat: show the renderings as an aligned, labelled list

The result box appended raw text, so several renderings would have arrived as one
paragraph with tab characters and labels buried inside it. Rows are now real
elements, keyed by tone index and written rather than appended, which is what
`translate:tone` carrying a row's full text buys.

The grid sits on the container rather than on each row, and rows use
`display: contents` to pass their spans up to it. That is what makes the label
column as wide as the widest label in the set instead of per-row, so every
translation starts at the same left edge and the list can be scanned down its
second column. Labels are held to the top of their row so a wrapped translation
doesn't leave its label floating in the middle.

Making #output a grid moved the error message into an element of its own: bare
textContent would have become an anonymous grid item and been squeezed into the
label column.

Rejected: one grid per row | each row's label column would size itself, so the translations wouldn't line up — the thing that makes the list scannable
Rejected: a table | the same alignment for markup that then has to be talked out of its default spacing, in a box whose rows are not tabular data
Confidence: medium
Not-tested: nothing beyond `pnpm build`'s type check if the human hasn't opened the window yet — say so plainly. Once they have, replace this line with what they confirmed, in the form: verified in the window that labels land before their translations, that the translations share a left edge, that the box scrolls at its cap, and that a bad `TONEMATE_MODEL` still shows one red full-width line

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

## Verification of the whole feature

From `src-tauri/`: `cargo test` (11 parser tests), `cargo clippy --lib --examples --tests`, `cargo run --example translate -- "我明天不能来了"` (labelled rows off the live model).
From the repo root: `pnpm build` (type-check), then `pnpm tauri dev` and a human at the keyboard for the window itself.
