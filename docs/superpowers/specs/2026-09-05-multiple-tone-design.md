# Multiple tones per translation

## Problem

tonemate currently answers one input with one translation. A single rendering
hides the choice that actually matters: the same sentence can be blunt or
deferential, and which one is right depends on who is reading it. The user has
to re-ask with an added instruction ("more polite") to see the alternative.

## Goal

For each input, infer what the speaker is trying to accomplish, then return 3-5
translations that differ in tone, register, and directness — each labelled, each
the right choice in a different situation. Show them as a read-only list under
the input.

## Out of scope

- Selecting a tone with the arrow keys and copying it with Enter (a later change)
- Showing the inferred intent on screen
- A fixed, configurable set of tone labels
- Splitting long inputs into segments

The first of these is the likely next step, so nothing here should make a
selected row hard to add: rows are addressed by index and rendered as discrete
elements rather than as one text blob.

## Decisions

**Labels are always Chinese**, whichever direction the translation runs. The
label exists so the reader can pick, and the reader is the person who typed the
input.

**The model decides how many**, between 3 and 5. Five tones that read alike are
worse than three that don't, so the count follows the input rather than a
constant.

**One streamed call, line-delimited output.** The alternatives were structured
JSON output and one call per tone. JSON is the sturdiest format but cannot be
parsed until it is complete, which costs the streaming that makes the app feel
immediate — several hundred tokens of dead time behind the pulsing dots. One
call per tone streams every row at once, but needs a first call to pick the
tones (delaying the fan-out), multiplies token cost by five, and introduces
concurrency and partial-failure handling. Line-delimited text keeps the single
call, needs no JSON escaping, and pins the label the moment its tab arrives, so
each row still shows its translation forming character by character.

## Architecture

```
bedrock.rs  ── text fragments ──▶  tones.rs  ── Tone updates ──▶  lib.rs
(talks to Bedrock)                 (parses)                       (emits events)
                                                                      │
                                                          translate:tone
                                                                      ▼
                                                                  main.ts
                                                             (upserts rows)
```

`bedrock.rs` keeps its current responsibility: make one streamed Converse call
and hand out text fragments. The new `tones.rs` turns that byte stream into row
updates. `lib.rs` wires the two together and emits events. The split matters for
testing: `tones.rs` is a pure function over strings, so it is the one piece of
this feature that can be tested without AWS credentials or a running window.

### tones.rs

```rust
#[derive(serde::Serialize)]
pub struct Tone {
    pub index: usize,
    pub label: String,
    pub text: String,
}

pub struct Parser { /* unclosed line, count of rows emitted */ }

impl Parser {
    /// Rows this fragment changed. A fragment carrying a newline touches two.
    pub fn push(&mut self, fragment: &str) -> Vec<Tone>;
    /// The last line has no trailing newline; and if nothing parsed at all,
    /// the whole raw output becomes one unlabelled row.
    pub fn finish(&mut self) -> Vec<Tone>;
}
```

`Tone::text` is the row's full text so far, not a delta. The frontend overwrites
by index, so it keeps no accumulation state and a repeated or reordered event
cannot corrupt a row. The payload is one sentence; resending it is cheaper than
the state a delta protocol would need on both sides.

### Parsing rules

- Buffer incoming text; split rows on `\n`, trimming a trailing `\r`
- The first `\t` in a row separates label from translation; the label is
  trimmed, and any later `\t` stays in the translation
- Before the `\t` arrives the label is unknown, so the row emits nothing
- Blank rows are skipped and do not consume an index
- A row with no `\t` — the model ignored the format — becomes an unlabelled row:
  empty `label`, the whole line as `text`
- At end of stream an unclosed final row is closed by the same rules, since the
  model usually omits the trailing newline
- If no row parsed at all, `finish` yields the entire raw output as one
  unlabelled row

Malformed output degrades to showing the text rather than to an error. A
translation the user can read beats a parser complaint.

### Events

| Event | Change | Payload |
|---|---|---|
| `translate:start` | unchanged | — |
| `translate:tone` | new, replaces `translate:delta` | `{ index, label, text }` |
| `translate:done` | unchanged | — |
| `translate:error` | unchanged | error string |

`translate:error` still replaces every row: a half-finished set of tones cannot
be told apart from a complete one.

### Prompt

`SYSTEM_PROMPT` keeps deciding the direction itself — the existing reasoning
holds, since text that mixes scripts would defeat any threshold Rust could
apply. Added instructions:

- Infer the speaker's intent: what they want to accomplish, their relationship
  to the reader, how blunt the original was
- Give 3-5 renderings that differ in tone, register, and directness, each the
  best choice in some concrete situation; if only three are meaningfully
  different, give three
- The first line is the most faithful, most neutral rendering; later lines
  increase in tonal distance
- Format: one row per line, `LABEL<TAB>TRANSLATION`. Labels in Chinese, 2-4
  characters. No numbering, no blank lines, no markdown, no explanation, and no
  line breaks inside a translation

`max_tokens` rises from 2048 to 4096: the same input now produces four or five
renderings, and the old budget truncates long inputs that used to fit. `effort`
stays `low`; inferring intent is heavier than a single literal translation, so
whether `low` still suffices is a question for the example run, not for this
document.

### Terminal output

`[tonemate] out: ` becomes a bare `[tonemate] out:` line and the raw fragments
stream onto the lines below it. Multi-row output then reads correctly on stdout
with no change to the printing code. `examples/translate.rs` matches.

### Frontend

`#output` changes from a div holding text to a container holding rows built by
JS. Translations must align across rows, so the container is the grid and each
row uses `display: contents` to hand its label and text to the container's
columns:

```css
#output { display: grid; grid-template-columns: max-content 1fr;
          column-gap: 10px; row-gap: 6px; }
.tone   { display: contents; }
.label  { align-self: start; font-size: 13px; white-space: nowrap;
          color: rgba(242, 242, 247, 0.45); user-select: none; }
```

`max-content` sizes the label column to the widest label, aligning every
translation. `align-self: start` keeps a label against its translation's first
line instead of centred against a wrapped block. `#output` keeps its
`max-height: 240px`, `overflow-y: auto`, and `user-select: text`; the font size,
line height, `pre-wrap`, and `overflow-wrap` move to `.text`. An unlabelled row
gets no label element and spans both columns via
`.tone.unlabeled .text { grid-column: 1 / -1 }`.

`main.ts` upserts on `translate:tone`: build the row if that index has none,
otherwise overwrite its label and text. `translate:start` and `resetOutput`
empty the container. `syncWindowHeight()` is still called per update — it
already coalesces into a frame, so the cost does not change with row count.
Scroll-following on `scrollHeight` stays correct: it tracks the newest row.

One trap: the error path currently writes `textContent` straight onto
`#output`. Once that element is a grid, bare text becomes an anonymous grid item
and lands in the `max-content` column, squeezed narrow. The error must go into a
column-spanning element instead.

## Testing

`tones.rs` gets unit tests, written before the parser:

- A whole two-row output in one fragment
- The same output fed one character at a time — identical result. `\t` and `\n`
  landing on fragment boundaries is where this parser is most likely to break
- Labels padded with spaces
- A blank line between rows
- No trailing newline
- A row missing its `\t`
- Output with no `\t` and no `\n` at all
- A translation containing a `\t`

Beyond that: `cargo test`, then
`cargo run --example translate -- "我明天不能来了"` against the real model to
check format compliance, time to first word, total time, and whether `low`
effort holds up; then `pnpm build` for type-checking. The window itself has to
be checked by hand — this environment cannot send keystrokes to the GUI.

README's description of the behaviour is updated to match.
