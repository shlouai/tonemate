//! Translation through an OpenAI-compatible chat-completions endpoint.
//!
//! Not "the Kimi client": Kimi, DeepSeek and OpenAI differ only in base URL,
//! model id, and a handful of body fields. Parameterising those three keeps the
//! SSE decoding — the part with all the logic — in one place instead of three.

use std::sync::OnceLock;

use futures_util::StreamExt;
use serde_json::{json, Value};

use super::sse::Decoder;

/// Built on first use so startup stays instant, then reused so later
/// translations skip the TLS handshake. The key travels per request, so one
/// client serves every endpoint and a key change needs no rebuild.
///
/// `OnceLock` rather than the `tokio::sync::OnceCell` Bedrock uses: building a
/// `reqwest::Client` awaits nothing, so there is no reason for the initialiser
/// to be async.
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Where to send the request and what to ask for.
pub struct Endpoint {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// Provider-specific body fields, merged over the common ones. Kimi uses
    /// this to switch reasoning off, which it must.
    pub extra: Value,
}

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(reqwest::Client::new)
}

/// One streamed chat completion. Returns everything the model said and why it
/// stopped; `on_delta` sees each fragment as it lands, so callers can show the
/// translation forming instead of waiting for the whole response.
pub(super) async fn converse(
    endpoint: &Endpoint,
    system_prompt: &str,
    text: &str,
    max_tokens: u32,
    mut on_delta: impl FnMut(&str),
) -> Result<(String, Option<String>), String> {
    let mut body = json!({
        "model": endpoint.model,
        "stream": true,
        // `max_tokens` is deprecated by Moonshot in favour of this.
        "max_completion_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": text },
        ],
    });

    // Merged rather than nested, so a provider can also override a common field
    // if it ever needs to.
    if let (Some(target), Some(extra)) = (body.as_object_mut(), endpoint.extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }

    let response = client()
        .post(format!("{}/chat/completions", endpoint.base_url))
        .bearer_auth(&endpoint.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    // A failure arrives as a normal JSON body, not as a stream. The provider's
    // own message is the useful part — "Invalid Authentication" is what tells a
    // user their key belongs to the other host.
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|json| {
                json.pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("HTTP {status}: {}", body.trim()));
        return Err(message);
    }

    let mut decoder = Decoder::default();
    let mut collected = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| err.to_string())?;
        for fragment in decoder.push(&chunk)? {
            collected.push_str(&fragment);
            on_delta(&fragment);
        }
    }

    // The two ways this endpoint returns 200 and no translation, each named so
    // the bar can show something better than a blank box.
    if collected.trim().is_empty() {
        if decoder.saw_reasoning() {
            return Err(
                "the model spent its whole budget reasoning and returned no translation — \
                 reasoning must be switched off for this model"
                    .to_string(),
            );
        }
        return Err(format!(
            "model returned no text (finish reason: {})",
            decoder.finish_reason().unwrap_or("unknown")
        ));
    }

    Ok((collected, decoder.finish_reason().map(str::to_string)))
}
