//! Which service translates, and the one thing every service is asked.
//!
//! The prompt lives here rather than in a transport because it is what decides
//! output quality: two providers holding their own copy would drift, and the
//! drift would look like a model difference.

pub(crate) mod bedrock;
pub(crate) mod openai_compat;
mod sse;

use tauri::AppHandle;

/// The direction is the model's call, not a character-class check in Rust: it
/// already reads the text, and input that mixes scripts — a Chinese sentence
/// carrying one English word — would fool any threshold we picked.
///
/// The tab-delimited line format is what lets the answer stream: each label is
/// fixed the moment its tab arrives, so a rendering fills in character by
/// character instead of appearing all at once at the end of the response.
pub(crate) const SYSTEM_PROMPT: &str =
    "You are a translation engine. If the user's text is English, translate \
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

/// The domestic host. A key issued for the international one
/// (`https://api.moonshot.ai/v1`) returns `401 Invalid Authentication` here and
/// vice versa, and the key string does not say which it is — hence the override.
const KIMI_BASE_URL: &str = "https://api.moonshot.cn/v1";

/// `kimi-k2.6` is the only Kimi model whose reasoning can be switched off;
/// `kimi-k2.7-code` rejects `thinking: disabled` with "only type=enabled is
/// allowed for this model", which is why it is not an option here.
const KIMI_MODEL: &str = "kimi-k2.6";

/// The service a translation goes through. An enum rather than a trait: the set
/// is fixed at compile time and picked by a `match`, so the boxing and lifetime
/// work an `async` trait method would need buys nothing.
///
/// Deliberately does not derive `Debug`, because the Kimi variant holds a live
/// API key and a derived `Debug` would put it in any log line that formatted
/// the value.
pub enum Provider {
    Bedrock,
    Kimi(String),
}

/// Which provider a stored setting pair means. Split out from `from_settings`
/// so the fallback rule can be tested: an `AppHandle` cannot be built in a
/// unit test, and this rule is the one the user notices when it is wrong.
fn choose(provider: &str, kimi_key: &str) -> Provider {
    match provider {
        "kimi" if kimi_key.is_empty() => {
            println!("[tonemate] kimi selected but no api key set; using bedrock");
            Provider::Bedrock
        }
        "kimi" => Provider::Kimi(kimi_key.to_string()),
        _ => Provider::Bedrock,
    }
}

impl Provider {
    /// What the user chose, or Bedrock when they chose nothing usable. Read per
    /// call rather than cached, so a change in the settings window takes effect
    /// on the next translation rather than the next launch.
    ///
    /// A provider selected without a key falls back rather than failing: an
    /// empty key means "not configured yet", which is not the same as a key the
    /// service rejected.
    pub fn from_settings(app: &AppHandle) -> Self {
        choose(
            &crate::settings::provider_name(app),
            &crate::settings::kimi_api_key(app),
        )
    }

    /// The provider `examples/translate.rs` should use. It has no `AppHandle`,
    /// so it configures itself from the environment instead of from settings.
    pub fn from_env() -> Self {
        match std::env::var("TONEMATE_KIMI_API_KEY") {
            Ok(key) if !key.is_empty() => Provider::Kimi(key),
            _ => Provider::Bedrock,
        }
    }

    /// Names the service in a log line or an error message. With more than one
    /// provider, "API key not valid" does not say whose.
    pub fn label(&self) -> &'static str {
        match self {
            Provider::Bedrock => "bedrock",
            Provider::Kimi(_) => "kimi",
        }
    }

    /// Where a Kimi request goes and what it asks for.
    ///
    /// `thinking: disabled` is not optional. Left on, reasoning tokens come out
    /// of the same budget as the answer: measured at 21.6s for 1023 reasoning
    /// frames, zero content frames and `finish_reason: length` — twenty-one
    /// seconds for no translation at all.
    fn kimi_endpoint(key: &str) -> openai_compat::Endpoint {
        openai_compat::Endpoint {
            base_url: env_or("TONEMATE_KIMI_BASE_URL", KIMI_BASE_URL),
            model: env_or("TONEMATE_KIMI_MODEL", KIMI_MODEL),
            api_key: key.to_string(),
            extra: serde_json::json!({ "thinking": { "type": "disabled" } }),
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
    let result = match provider {
        Provider::Bedrock => match bedrock::converse(SYSTEM_PROMPT, "hi", 1, |_| {}).await {
            Ok(_) => Ok(()),
            Err(err) if err.starts_with("model returned no text") => Ok(()),
            Err(err) => Err(err),
        },
        // A one-token budget makes the answer empty by construction, which the
        // transport would otherwise call a failure. Only reachability is being
        // tested here, so that error is the success case.
        Provider::Kimi(key) => {
            match openai_compat::converse(
                &Provider::kimi_endpoint(key),
                SYSTEM_PROMPT,
                "hi",
                1,
                |_| {},
            )
            .await
            {
                Ok(_) => Ok(()),
                Err(err) if err.starts_with("model returned no text") => Ok(()),
                Err(err) => Err(err),
            }
        }
    };

    result.map_err(|err| format!("{}: {err}", provider.label()))
}

pub async fn translate(
    provider: &Provider,
    text: &str,
    on_delta: impl FnMut(&str),
) -> Result<(), String> {
    let result = match provider {
        Provider::Bedrock => bedrock::converse(SYSTEM_PROMPT, text, MAX_TOKENS, on_delta).await,
        Provider::Kimi(key) => {
            openai_compat::converse(
                &Provider::kimi_endpoint(key),
                SYSTEM_PROMPT,
                text,
                MAX_TOKENS as u32,
                on_delta,
            )
            .await
        }
    };

    // Prefixed here rather than in each transport, so every provider's failures
    // read the same way in the bar and in the log.
    result.map(|_| ()).map_err(|err| format!("{}: {err}", provider.label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_with_a_key_uses_kimi() {
        let provider = choose("kimi", "sk-test123");
        match &provider {
            Provider::Kimi(key) => assert_eq!(key, "sk-test123"),
            _ => panic!("expected Kimi variant"),
        }
        assert_eq!(provider.label(), "kimi");
    }

    #[test]
    fn kimi_without_a_key_falls_back_to_bedrock() {
        let provider = choose("kimi", "");
        match provider {
            Provider::Bedrock => (),
            _ => panic!("expected Bedrock fallback"),
        }
        assert_eq!(provider.label(), "bedrock");
    }

    #[test]
    fn an_unknown_provider_name_uses_bedrock() {
        let provider = choose("deepseek", "");
        match provider {
            Provider::Bedrock => (),
            _ => panic!("expected Bedrock fallback"),
        }
        assert_eq!(provider.label(), "bedrock");
    }
}
