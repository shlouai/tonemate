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
