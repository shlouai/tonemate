//! Which service translates, and the one thing every service is asked.
//!
//! The prompt lives here rather than in a transport because it is what decides
//! output quality: two providers holding their own copy would drift, and the
//! drift would look like a model difference.

pub(crate) mod bedrock;
pub(crate) mod openai_compat;
mod sse;

use tauri::AppHandle;

/// Which way a translation runs. Decided in Rust rather than inferred by the
/// model, because the model cannot be made to infer it reliably: `kimi-k2.6`
/// pins `temperature` at 0.6 and rejects any other value, so its obedience to an
/// inferred rule is probabilistic by construction. Measured with the direction
/// left to the model, Chinese input carrying English words came back in the
/// wrong language in 5 of 24 runs — and sometimes mixed both languages inside a
/// single response, one row English and the next Chinese.
///
/// Naming the target removes the inference instead of making it likelier.
#[derive(Debug, PartialEq)]
enum Direction {
    /// The user typed Chinese, so every line must come back English.
    FromChinese,
    /// No Chinese present. Which way this goes is genuinely the model's call:
    /// telling English from French in Rust needs a language identifier, and
    /// guessing wrong would be worse than the ambiguity. The documented rule —
    /// English in gets Chinese back, anything else gets English — is handed to
    /// the model intact, and no failure was observed on this path.
    Other,
}

impl Direction {
    /// Presence of one Han character, not a proportion of them.
    ///
    /// This is what the old "any threshold would be fooled" objection was really
    /// about, and it is right about ratios: `帮我 review 一下这个 PR` is 43% Han
    /// and `这个 feature 的 deadline 是下周五` is 32%, yet both are plainly
    /// Chinese sentences borrowing English nouns. Presence has no threshold to
    /// tune and gets both right — a Chinese speaker reaching for an English term
    /// is still writing Chinese and still wants English back.
    fn detect(text: &str) -> Self {
        if text.chars().any(is_han) {
            Direction::FromChinese
        } else {
            Direction::Other
        }
    }

    /// Stated as a command about every line, because the failure being fixed was
    /// per-line rather than per-response.
    fn instruction(&self) -> &'static str {
        match self {
            Direction::FromChinese => {
                "The user's text is Chinese: every line you output must be in English."
            }
            Direction::Other => {
                "The user's text is not Chinese. If it is English, every line you output must be \
                 in Chinese; otherwise every line you output must be in English."
            }
        }
    }
}

/// The Han blocks that matter here: the common ideographs, Extension A, and the
/// compatibility ideographs. Kana is deliberately absent — this app translates
/// between Chinese and English, and Japanese text usually carries kanji anyway.
fn is_han(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}'   // CJK Extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
    )
}

/// The prompt for one input: the target language, the shared instructions, then
/// the target language again. Repeated deliberately — a single mention at the
/// top was what the model drifted from mid-response.
pub(crate) fn prompt_for(text: &str) -> String {
    let instruction = Direction::detect(text).instruction();
    format!("You are a translation engine. {instruction}\n{SYSTEM_PROMPT_BODY}\n{instruction}")
}

/// The tab-delimited line format is what lets the answer stream: each label is
/// fixed the moment its tab arrives, so a rendering fills in character by
/// character instead of appearing all at once at the end of the response.
///
/// Three things here are stricter than they look like they need to be, and each
/// is a failure measured against a real model rather than a precaution:
///
/// 1. **The direction rule comes first and is stated as a prohibition.** As one
///    clause competing with a longer instruction about tonal variety, it lost:
///    Kimi answered `我明天不能来了` with five Chinese paraphrases in 6 of 20
///    runs, because varying the tone of the original is a plausible reading of
///    the rest of the prompt.
/// 2. **The label is defined as naming the tone, and never a paraphrase.**
///    Otherwise it drifts into restating the input — `算了吧` came back labelled
///    算了 / 罢了 / 罢了 / 拉倒, which makes the column useless for choosing a
///    rendering, in 7 of 20 runs.
/// 3. **The separator is a real tab, shown in a worked example.** An earlier
///    version named it in backslash-t notation, and the model sometimes emitted
///    those two characters literally; `tones::Parser` splits on U+0009 alone, so
///    such a row rendered with its label welded onto the front of the text.
///
/// With all three, 40 consecutive Kimi runs across eight inputs produced no
/// wrong-direction line, no duplicated label, and no malformed separator.
const SYSTEM_PROMPT_BODY: &str =
    "Restating the input in its own language is always wrong, however good the restatement.\n\
     Work out what the writer is doing: what they want from the reader, how they stand in relation \
     to that reader, and how blunt the original was. Then give 3 to 5 translations that differ in \
     tone, register, and directness, each the right choice in some concrete situation. If only \
     three are meaningfully different, give three — never pad the list with near-duplicates.\n\
     The first line is the most faithful, most neutral rendering. Each later line sits further \
     from it in tone.\n\
     Each line is: a label, then one TAB character, then the translation.\n\
     The label is 2 to 4 Chinese characters naming the TONE — 直白, 委婉, 正式, 客气, 冷淡, 随口 \
     and the like. It describes how the line sounds. It is never a translation, never a paraphrase \
     of the input, and never repeated on another line.\n\
     Separate label from translation with one real TAB character (U+0009) and nothing else. Never \
     write the two characters backslash-t. Never use a space, fullwidth space, colon, or dash.\n\
     Example of one correct line:\n\
     委婉\tI'm afraid I can't make it tomorrow.\n\
     No numbering, no blank lines, no markdown, no quotes, no explanation, and no line break \
     inside a translation.";
// The `\t` in the example line above is an escape sequence, so it compiles to one real tab while
// staying visible in this source. An earlier version wrote `\\t` — two characters — on the theory
// that a tab would be invisible here; that confused a pasted tab byte with the escape, and cost us
// a model that copied the notation instead of obeying it.

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

/// DeepSeek's host. The `/v1` suffix is optional in their docs but kept to
/// match Kimi's shape and the `/chat/completions` the transport appends.
const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";

/// `deepseek-chat` is the non-reasoning model. `deepseek-reasoner` is a
/// reasoning model, and is deliberately not an option here for the same reason
/// Kimi's reasoning is switched off: it would spend the whole budget deliberating
/// and return no translation.
const DEEPSEEK_MODEL: &str = "deepseek-chat";

/// The service a translation goes through. An enum rather than a trait: the set
/// is fixed at compile time and picked by a `match`, so the boxing and lifetime
/// work an `async` trait method would need buys nothing.
///
/// Deliberately does not derive `Debug`, because the Kimi variant holds a live
/// API key and a derived `Debug` would put it in any log line that formatted
/// the value.
pub enum Provider {
    Bedrock(String),
    Kimi(String),
    DeepSeek(String),
}

/// Which provider a stored setting pair means. Split out from `from_settings`
/// so the fallback rule can be tested: an `AppHandle` cannot be built in a
/// unit test, and this rule is the one the user notices when it is wrong.
fn choose(provider: &str, kimi_key: &str, deepseek_key: &str, aws_profile: &str) -> Provider {
    match provider {
        "kimi" if kimi_key.is_empty() => {
            println!("[tonemate] kimi selected but no api key set; using bedrock");
            Provider::Bedrock(aws_profile.to_string())
        }
        "kimi" => Provider::Kimi(kimi_key.to_string()),
        "deepseek" if deepseek_key.is_empty() => {
            println!("[tonemate] deepseek selected but no api key set; using bedrock");
            Provider::Bedrock(aws_profile.to_string())
        }
        "deepseek" => Provider::DeepSeek(deepseek_key.to_string()),
        "bedrock" => Provider::Bedrock(aws_profile.to_string()),
        _ => {
            println!("[tonemate] unrecognised provider \"{provider}\"; using bedrock");
            Provider::Bedrock(aws_profile.to_string())
        }
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
            &crate::settings::deepseek_api_key(app),
            &crate::settings::aws_profile(app),
        )
    }

    /// The provider `examples/translate.rs` should use. It has no `AppHandle`,
    /// so it configures itself from the environment instead of from settings.
    pub fn from_env() -> Self {
        match std::env::var("TONEMATE_KIMI_API_KEY") {
            Ok(key) if !key.is_empty() => Provider::Kimi(key),
            _ => match std::env::var("TONEMATE_DEEPSEEK_API_KEY") {
                Ok(key) if !key.is_empty() => Provider::DeepSeek(key),
                _ => Provider::Bedrock(env_or("TONEMATE_AWS_PROFILE", "")),
            },
        }
    }

    /// Names the service in a log line or an error message. With more than one
    /// provider, "API key not valid" does not say whose.
    pub fn label(&self) -> &'static str {
        match self {
            Provider::Bedrock(_) => "bedrock",
            Provider::Kimi(_) => "kimi",
            Provider::DeepSeek(_) => "deepseek",
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
            max_tokens_field: "max_completion_tokens",
            extra: serde_json::json!({ "thinking": { "type": "disabled" } }),
        }
    }

    /// Where a DeepSeek request goes and what it asks for. `extra` is empty:
    /// `deepseek-chat` is non-reasoning, so there is no `thinking` to switch off.
    fn deepseek_endpoint(key: &str) -> openai_compat::Endpoint {
        openai_compat::Endpoint {
            base_url: env_or("TONEMATE_DEEPSEEK_BASE_URL", DEEPSEEK_BASE_URL),
            model: env_or("TONEMATE_DEEPSEEK_MODEL", DEEPSEEK_MODEL),
            api_key: key.to_string(),
            max_tokens_field: "max_tokens",
            extra: serde_json::json!({}),
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
        Provider::Bedrock(profile) => {
            match bedrock::converse(profile, &prompt_for("hi"), "hi", 1, |_| {}).await {
                Ok(_) => Ok(()),
                Err(err) if err.starts_with("model returned no text") => Ok(()),
                Err(err) => Err(err),
            }
        }
        // A one-token budget makes the answer empty by construction, which the
        // transport would otherwise call a failure. Only reachability is being
        // tested here, so that error is the success case.
        Provider::Kimi(key) => {
            match openai_compat::converse(
                &Provider::kimi_endpoint(key),
                &prompt_for("hi"),
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
        Provider::DeepSeek(key) => {
            match openai_compat::converse(
                &Provider::deepseek_endpoint(key),
                &prompt_for("hi"),
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
        Provider::Bedrock(profile) => {
            bedrock::converse(profile, &prompt_for(text), text, MAX_TOKENS, on_delta).await
        }
        Provider::Kimi(key) => {
            openai_compat::converse(
                &Provider::kimi_endpoint(key),
                &prompt_for(text),
                text,
                MAX_TOKENS as u32,
                on_delta,
            )
            .await
        }
        Provider::DeepSeek(key) => {
            openai_compat::converse(
                &Provider::deepseek_endpoint(key),
                &prompt_for(text),
                text,
                MAX_TOKENS as u32,
                on_delta,
            )
            .await
        }
    };

    // Prefixed here rather than in each transport, so every provider's failures
    // read the same way in the bar and in the log.
    result
        .map(|_| ())
        .map_err(|err| format!("{}: {err}", provider.label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_with_a_key_uses_kimi() {
        let provider = choose("kimi", "sk-test123", "", "");
        match &provider {
            Provider::Kimi(key) => assert_eq!(key, "sk-test123"),
            _ => panic!("expected Kimi variant"),
        }
        assert_eq!(provider.label(), "kimi");
    }

    #[test]
    fn kimi_without_a_key_falls_back_to_bedrock() {
        let provider = choose("kimi", "", "", "");
        match provider {
            Provider::Bedrock(_) => (),
            _ => panic!("expected Bedrock fallback"),
        }
        assert_eq!(provider.label(), "bedrock");
    }

    #[test]
    fn deepseek_with_a_key_uses_deepseek() {
        let provider = choose("deepseek", "", "sk-test456", "");
        match &provider {
            Provider::DeepSeek(key) => assert_eq!(key, "sk-test456"),
            _ => panic!("expected DeepSeek variant"),
        }
        assert_eq!(provider.label(), "deepseek");
    }

    #[test]
    fn deepseek_without_a_key_falls_back_to_bedrock() {
        let provider = choose("deepseek", "", "", "");
        match provider {
            Provider::Bedrock(_) => (),
            _ => panic!("expected Bedrock fallback"),
        }
        assert_eq!(provider.label(), "bedrock");
    }

    #[test]
    fn an_unknown_provider_name_uses_bedrock() {
        let provider = choose("openai", "", "", "");
        match provider {
            Provider::Bedrock(_) => (),
            _ => panic!("expected Bedrock fallback"),
        }
        assert_eq!(provider.label(), "bedrock");
    }

    #[test]
    fn the_aws_profile_rides_along_with_bedrock() {
        let provider = choose("bedrock", "", "", "corp");
        match provider {
            Provider::Bedrock(profile) => assert_eq!(profile, "corp"),
            _ => panic!("expected Bedrock with the stored profile"),
        }
    }

    #[test]
    fn chinese_input_is_translated_into_english() {
        assert_eq!(Direction::detect("我明天不能来了"), Direction::FromChinese);
    }

    /// The failure this whole mechanism exists for. Both of these are ordinary
    /// Chinese sentences borrowing English nouns, and both are under half Han by
    /// character count — 43% and 32% — so a ratio threshold calls them English
    /// and sends the translation the wrong way.
    #[test]
    fn chinese_carrying_english_words_is_still_chinese() {
        for text in [
            "帮我 review 一下这个 PR。",
            "这个 feature 的 deadline 是下周五。",
            "server 挂了,你能 restart 一下吗?",
            "把 log 发我看看。",
            "这个 bug 在 production 环境下才会出现,本地跑不出来。",
        ] {
            assert_eq!(
                Direction::detect(text),
                Direction::FromChinese,
                "{text:?} should translate into English"
            );
        }
    }

    #[test]
    fn text_with_no_chinese_is_left_to_the_model() {
        for text in [
            "Could you review this by Friday?",
            "Bonjour, comment allez-vous?",
            "",
            "42",
        ] {
            assert_eq!(Direction::detect(text), Direction::Other, "{text:?}");
        }
    }

    /// Han lives in several blocks; a rare character must not read as "no
    /// Chinese here" and silently flip the direction.
    #[test]
    fn han_outside_the_common_block_still_counts() {
        // 㐀 is CJK Extension A, 豈 is a Compatibility Ideograph.
        assert_eq!(Direction::detect("㐀"), Direction::FromChinese);
        assert_eq!(Direction::detect("豈"), Direction::FromChinese);
    }

    /// The instruction has to be unmissable, because the model was observed
    /// mixing both languages inside a single response.
    #[test]
    fn the_prompt_names_the_target_language_twice() {
        let prompt = prompt_for("我明天不能来了");
        assert_eq!(
            prompt
                .matches("every line you output must be in English")
                .count(),
            2,
            "the target should be stated up front and restated at the end"
        );
        assert!(!prompt.contains("output must be Chinese"));
    }

    /// Non-Chinese input keeps the behaviour the README documents: English in
    /// gets Chinese back, anything else gets English.
    #[test]
    fn non_chinese_input_keeps_the_documented_two_way_rule() {
        let prompt = prompt_for("Could you review this by Friday?");
        assert!(prompt.contains("If it is English, every line you output must be in Chinese"));
        assert!(prompt.contains("otherwise every line you output must be in English"));
    }
}
