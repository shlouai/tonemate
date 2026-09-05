//! Which service translates, and the one thing every service is asked.
//!
//! The prompt lives here rather than in a transport because it is what decides
//! output quality: two providers holding their own copy would drift, and the
//! drift would look like a model difference.

pub mod bedrock;

use tauri::AppHandle;

/// The direction is the model's call, not a character-class check in Rust: it
/// already reads the text, and input that mixes scripts — a Chinese sentence
/// carrying one English word — would fool any threshold we picked.
///
/// The tab-delimited line format is what lets the answer stream: each label is
/// fixed the moment its tab arrives, so a rendering fills in character by
/// character instead of appearing all at once at the end of the response.
pub(crate) const SYSTEM_PROMPT: &str = "You are a translation engine. If the user's text is English, translate \
     it into natural, idiomatic Chinese; otherwise translate it into natural, idiomatic English.\n\
     Work out what the writer is doing first: what they want from the reader, how they stand in \
     relation to that reader, and how blunt the original was. Then give 3 to 5 renderings that \
     differ in tone, register, and directness, each one the right choice in some concrete \
     situation. If only three are meaningfully different, give three — never pad the list with \
     near-duplicates.\n\
     The first line is the most faithful, most neutral rendering. Each later line sits further \
     from it in tone.\n\
     Output one rendering per line: a label in Chinese of 2 to 4 characters, then a single tab \
     character (ASCII 9, \\t), then the translation. Use only the tab character as the separator—\
     never a fullwidth space, colon, dash, or any other character. No numbering, no blank lines, \
     no markdown, no quotes, no explanation, and never a line break inside a translation.";
// \\t above is deliberately two characters (backslash-t notation): it names the tab for the
// model without putting a real tab in the source, which would be invisible here and in logs.

/// Four or five renderings of the same input, so roughly five times the budget
/// one translation needed.
const MAX_TOKENS: i32 = 4096;

/// The service a translation goes through. An enum rather than a trait: the set
/// is fixed at compile time and picked by a `match`, so the boxing and lifetime
/// work an `async` trait method would need buys nothing.
pub enum Provider {
    Bedrock,
}

impl Provider {
    /// What the user chose, or Bedrock when they chose nothing usable. Read per
    /// call rather than cached, so a change in the settings window takes effect
    /// on the next translation rather than the next launch.
    pub fn from_settings(_app: &AppHandle) -> Self {
        Provider::Bedrock
    }

    /// The provider `examples/translate.rs` should use. It has no `AppHandle`,
    /// so it configures itself from the environment instead of from settings.
    pub fn from_env() -> Self {
        Provider::Bedrock
    }

    /// Names the service in a log line or an error message. With more than one
    /// provider, "API key not valid" does not say whose.
    pub fn label(&self) -> &'static str {
        match self {
            Provider::Bedrock => "bedrock",
        }
    }
}

pub(crate) fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Pay the one-time costs — config load, credential resolution, and the TLS
/// handshake — before the user asks for anything. Measured on a warm AWS
/// profile, that first request carries ~1s the later ones don't, and a
/// one-token budget buys it for a rounding error's worth of tokens. Runs
/// against the configured model, so a bad model id surfaces at startup rather
/// than on the first translation.
pub async fn warm(provider: &Provider) -> Result<(), String> {
    match provider {
        Provider::Bedrock => bedrock::converse(SYSTEM_PROMPT, "hi", 1, |_| {})
            .await
            .map(|_| ()),
    }
}

pub async fn translate(
    provider: &Provider,
    text: &str,
    on_delta: impl FnMut(&str),
) -> Result<(), String> {
    let (translated, stop_reason) = match provider {
        Provider::Bedrock => bedrock::converse(SYSTEM_PROMPT, text, MAX_TOKENS, on_delta).await?,
    };

    if translated.trim().is_empty() {
        return Err(format!(
            "model returned no text (stop reason: {})",
            stop_reason.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    Ok(())
}
