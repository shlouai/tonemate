//! Translation via Claude on Amazon Bedrock.
//!
//! The call lives in Rust rather than the webview for two reasons: the AWS
//! credential chain (`~/.aws/credentials`) is only reachable from the host
//! process, and the result has to land on the terminal's stdout.

use std::collections::HashMap;

use aws_config::{BehaviorVersion, Region};
use aws_sdk_bedrockruntime::{
    error::DisplayErrorContext,
    types::{
        ContentBlock, ConversationRole, ConverseStreamOutput, InferenceConfiguration, Message,
        SystemContentBlock,
    },
    Client,
};
use aws_smithy_types::Document;
use tokio::sync::OnceCell;

const DEFAULT_PROFILE: &str = "twdc-bedrock-central";
const DEFAULT_REGION: &str = "us-west-2";
/// Bedrock only serves the Claude 5 family through cross-region inference
/// profiles, hence the `us.` prefix — the bare `anthropic.claude-opus-5` id is
/// rejected for on-demand throughput.
const DEFAULT_MODEL: &str = "us.anthropic.claude-opus-5";
/// Adaptive thinking is on by default on Opus 5; `low` keeps a one-line
/// translation from turning into a multi-second reasoning pass.
const DEFAULT_EFFORT: &str = "low";

const SYSTEM_PROMPT: &str = "You are a translation engine. Translate the user's text into natural, \
     idiomatic English, preserving its tone and register. Output only the translation: no preamble, \
     no quotes, no explanation, no notes.";

/// Built on first use so startup stays instant, then reused so subsequent
/// translations skip credential resolution.
static CLIENT: OnceCell<Client> = OnceCell::const_new();

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// `effort` is an Opus/Sonnet-5-era parameter; Haiku 4.5 and older models
/// reject it. Setting `TONEMATE_EFFORT=` (empty) drops it from the request so
/// `TONEMATE_MODEL` can point at one of those.
fn effort() -> Option<String> {
    match std::env::var("TONEMATE_EFFORT") {
        Ok(value) if value.is_empty() => None,
        Ok(value) => Some(value),
        Err(_) => Some(DEFAULT_EFFORT.to_string()),
    }
}

async fn client() -> &'static Client {
    CLIENT
        .get_or_init(|| async {
            let config = aws_config::defaults(BehaviorVersion::latest())
                .profile_name(env_or("TONEMATE_AWS_PROFILE", DEFAULT_PROFILE))
                .region(Region::new(env_or("TONEMATE_AWS_REGION", DEFAULT_REGION)))
                .load()
                .await;
            Client::new(&config)
        })
        .await
}

/// One streamed Converse call. Returns everything the model said; `on_delta`
/// sees each text fragment as it lands, so callers can show the translation
/// forming instead of waiting for the full response.
async fn converse(
    text: &str,
    max_tokens: i32,
    mut on_delta: impl FnMut(&str),
) -> Result<(String, Option<String>), String> {
    let user_turn = Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text(text.to_string()))
        .build()
        .map_err(|err| err.to_string())?;

    let mut request = client()
        .await
        .converse_stream()
        .model_id(env_or("TONEMATE_MODEL", DEFAULT_MODEL))
        .system(SystemContentBlock::Text(SYSTEM_PROMPT.to_string()))
        .messages(user_turn)
        .inference_config(InferenceConfiguration::builder().max_tokens(max_tokens).build());

    if let Some(effort) = effort() {
        request = request.additional_model_request_fields(Document::Object(HashMap::from([(
            "output_config".to_string(),
            Document::Object(HashMap::from([(
                "effort".to_string(),
                Document::String(effort),
            )])),
        )])));
    }

    // Bedrock's own message ("The provided model identifier is invalid.") is the
    // useful part; `DisplayErrorContext` is the fallback for transport-level
    // failures, where there is no service message to read.
    let mut response = request.send().await.map_err(|err| {
        err.as_service_error()
            .and_then(|service_err| service_err.meta().message())
            .map(str::to_string)
            .unwrap_or_else(|| DisplayErrorContext(&err).to_string())
    })?;

    let mut collected = String::new();
    let mut stop_reason = None;

    loop {
        match response.stream.recv().await {
            Ok(Some(ConverseStreamOutput::ContentBlockDelta(event))) => {
                // `as_text` also filters out reasoning deltas, which adaptive
                // thinking emits alongside the answer.
                if let Some(fragment) = event.delta().and_then(|delta| delta.as_text().ok()) {
                    collected.push_str(fragment);
                    on_delta(fragment);
                }
            }
            Ok(Some(ConverseStreamOutput::MessageStop(event))) => {
                stop_reason = Some(format!("{:?}", event.stop_reason()));
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(err) => return Err(DisplayErrorContext(&err).to_string()),
        }
    }

    Ok((collected, stop_reason))
}

/// Pay the one-time costs — config load, credential resolution, and the TLS
/// handshake to Bedrock — before the user asks for anything. Measured on a
/// warm profile, that first request carries ~1s the later ones don't, and
/// `max_tokens: 1` buys it for a rounding error's worth of tokens. Runs against
/// the configured model, so a bad `TONEMATE_MODEL` surfaces at startup rather
/// than on the first translation.
pub async fn warm() -> Result<(), String> {
    converse("hi", 1, |_| {}).await.map(|_| ())
}

pub async fn translate(text: &str, on_delta: impl FnMut(&str)) -> Result<String, String> {
    let (translated, stop_reason) = converse(text, 2048, on_delta).await?;

    if translated.trim().is_empty() {
        return Err(format!(
            "model returned no text (stop reason: {})",
            stop_reason.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    Ok(translated.trim().to_string())
}
