//! Pulls answer text out of an OpenAI-compatible SSE stream.
//!
//! Bytes rather than `&str`, and one line at a time: a chunk off the socket can
//! split a multi-byte character, and every label this app asks the model for is
//! Chinese, so that is the ordinary case rather than the edge case. Buffering
//! bytes and decoding only whole lines makes the split impossible to get wrong.
//!
//! No network and no `async` here, which is the point — this is where the
//! stream format's every quirk is pinned down by a test.

use serde::Deserialize;

/// Only the fields that change what the app does. `serde` ignores the rest,
/// which is what lets a provider add to the shape without breaking us.
#[derive(Deserialize)]
struct Frame {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    /// Undocumented in the streaming schema but present in practice on
    /// reasoning-capable models.
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

#[derive(Default)]
pub struct Decoder {
    /// The line still arriving, up to but not including its newline.
    pending: Vec<u8>,
    /// Why the model stopped, once a frame says so. `length` means the budget
    /// ran out, which is the tell for reasoning left switched on.
    finish_reason: Option<String>,
    /// Whether any reasoning arrived, so an empty answer can say why it is
    /// empty instead of just being blank.
    saw_reasoning: bool,
}

impl Decoder {
    /// The answer fragments this chunk completed. A chunk may contain any number
    /// of whole frames plus a partial one, and all are handled. Frames that
    /// carry no answer text — the opening `role` frame, reasoning, the `[DONE]`
    /// sentinel, anything unrecognised — produce nothing.
    ///
    /// `Err` means the provider reported a failure mid-stream, which ends the
    /// translation: half a set of renderings cannot be told from a whole one.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        self.pending.extend_from_slice(chunk);
        let mut fragments = Vec::new();

        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=newline).collect();
            // Lossy rather than strict: a line that is not valid UTF-8 is a
            // provider bug, and dropping the translation over it would be worse
            // than showing a replacement character.
            let line = String::from_utf8_lossy(&line);
            self.line(line.trim_end_matches(['\n', '\r']), &mut fragments)?;
        }

        Ok(fragments)
    }

    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    pub fn saw_reasoning(&self) -> bool {
        self.saw_reasoning
    }

    /// One complete line. Everything that is not a `data:` payload belongs to
    /// the SSE framing — blank separators, `event:` names, `:` comments — and is
    /// none of this decoder's business.
    fn line(&mut self, line: &str, fragments: &mut Vec<String>) -> Result<(), String> {
        let Some(payload) = line.strip_prefix("data:") else {
            return Ok(());
        };
        let payload = payload.trim();

        // The sentinel is not JSON. The loop ends when the body does, so there
        // is nothing to do but decline to parse it.
        if payload == "[DONE]" {
            return Ok(());
        }

        // A frame this build cannot parse is skipped rather than fatal: a
        // provider adding a shape must not break a translation already arriving.
        let Ok(frame) = serde_json::from_str::<Frame>(payload) else {
            return Ok(());
        };

        if let Some(error) = frame.error {
            return Err(error.message);
        }

        for choice in frame.choices {
            if let Some(reason) = choice.finish_reason {
                self.finish_reason = Some(reason);
            }
            if choice.delta.reasoning_content.is_some() {
                self.saw_reasoning = true;
            }
            // An empty string is what the opening frame carries alongside the
            // role, and it is not a fragment of anything.
            if let Some(content) = choice.delta.content.filter(|text| !text.is_empty()) {
                fragments.push(content);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every fragment the decoder emitted across a whole stream.
    fn feed(chunks: &[&str]) -> Result<Vec<String>, String> {
        let mut decoder = Decoder::default();
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(decoder.push(chunk.as_bytes())?);
        }
        Ok(out)
    }

    fn frame(delta: &str) -> String {
        format!("data: {{\"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":null}}]}}\n\n")
    }

    #[test]
    fn content_deltas_become_fragments() {
        let stream = format!(
            "{}{}data: [DONE]\n\n",
            frame(r#"{"content":"直译\t"}"#),
            frame(r#"{"content":"I can't come."}"#)
        );
        assert_eq!(feed(&[&stream]).unwrap(), vec!["直译\t", "I can't come."]);
    }

    /// The opening frame carries a role and an empty string, which is not text.
    #[test]
    fn the_opening_role_frame_emits_nothing() {
        let stream = frame(r#"{"role":"assistant","content":""}"#);
        assert_eq!(feed(&[&stream]).unwrap(), Vec::<String>::new());
    }

    /// Kimi streams reasoning in the same shape as the answer. Letting it
    /// through would put the model's private deliberation in the result box.
    #[test]
    fn reasoning_deltas_are_skipped_but_remembered() {
        let mut decoder = Decoder::default();
        let stream = format!(
            "{}{}",
            frame(r#"{"reasoning_content":"The user wants"}"#),
            frame(r#"{"content":"直译\tHello."}"#)
        );
        assert_eq!(
            decoder.push(stream.as_bytes()).unwrap(),
            vec!["直译\tHello."]
        );
        assert!(decoder.saw_reasoning());
    }

    /// Where this decoder is most likely to break: the transport hands over
    /// whatever the socket gave it, which respects no boundary at all.
    #[test]
    fn a_frame_split_across_chunks_is_reassembled() {
        let whole = format!(
            "{}{}data: [DONE]\n\n",
            frame(r#"{"content":"直译\t"}"#),
            frame(r#"{"content":"晚上好"}"#)
        );
        let bytes = whole.as_bytes();
        // Split at every byte offset, including inside `data:`, inside the JSON,
        // and mid-character in the Chinese.
        for split in 1..bytes.len() {
            let mut decoder = Decoder::default();
            let mut out = Vec::new();
            out.extend(decoder.push(&bytes[..split]).unwrap());
            out.extend(decoder.push(&bytes[split..]).unwrap());
            assert_eq!(
                out.concat(),
                "直译\t晚上好",
                "split at byte {split} changed the result"
            );
        }
    }

    #[test]
    fn several_frames_in_one_chunk_all_arrive() {
        let stream = format!(
            "{}{}{}",
            frame(r#"{"content":"a"}"#),
            frame(r#"{"content":"b"}"#),
            frame(r#"{"content":"c"}"#)
        );
        assert_eq!(feed(&[&stream]).unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn carriage_returns_and_blank_lines_are_tolerated() {
        let stream = "\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        assert_eq!(feed(&[stream]).unwrap(), vec!["a"]);
    }

    /// Named events and comment lines belong to the SSE framing, not to us.
    #[test]
    fn non_data_lines_are_ignored() {
        let stream = format!(": keep-alive\nevent: message\n{}", frame(r#"{"content":"a"}"#));
        assert_eq!(feed(&[&stream]).unwrap(), vec!["a"]);
    }

    /// Forward compatibility: a shape this build does not recognise must not
    /// abort a translation that is otherwise arriving fine.
    #[test]
    fn unrecognised_frames_are_skipped() {
        let stream = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[]}}}}]}}\n\n{}",
            frame(r#"{"content":"a"}"#)
        );
        assert_eq!(feed(&[&stream]).unwrap(), vec!["a"]);
    }

    #[test]
    fn malformed_json_is_skipped() {
        let stream = format!("data: {{ not json\n\n{}", frame(r#"{"content":"a"}"#));
        assert_eq!(feed(&[&stream]).unwrap(), vec!["a"]);
    }

    /// Both error `type`s observed live: the wrong host, and an unpaid account.
    #[test]
    fn an_error_frame_becomes_an_error() {
        for (body, expected) in [
            (
                r#"{"error":{"message":"Invalid Authentication","type":"invalid_authentication_error"}}"#,
                "Invalid Authentication",
            ),
            (
                r#"{"error":{"message":"suspended due to insufficient balance","type":"exceeded_current_quota_error"}}"#,
                "suspended due to insufficient balance",
            ),
        ] {
            let stream = format!("data: {body}\n\n");
            assert_eq!(feed(&[&stream]), Err(expected.to_string()));
        }
    }

    #[test]
    fn the_finish_reason_is_captured() {
        let mut decoder = Decoder::default();
        let stream = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n";
        decoder.push(stream.as_bytes()).unwrap();
        assert_eq!(decoder.finish_reason(), Some("length"));
    }

    /// The failure Kimi produces when reasoning is left on: a full budget of
    /// deliberation and not one word of translation. It has to be reportable as
    /// something other than an empty box.
    #[test]
    fn a_reasoning_only_stream_yields_no_text_but_explains_itself() {
        let mut decoder = Decoder::default();
        let stream = format!(
            "{}data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"length\"}}]}}\n\n",
            frame(r#"{"reasoning_content":"thinking..."}"#)
        );
        assert_eq!(decoder.push(stream.as_bytes()).unwrap(), Vec::<String>::new());
        assert!(decoder.saw_reasoning());
        assert_eq!(decoder.finish_reason(), Some("length"));
    }
}
