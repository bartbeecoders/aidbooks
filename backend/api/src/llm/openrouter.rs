//! Thin OpenRouter chat-completions client with a built-in MOCK mode.
//!
//! When `Config.openrouter_api_key` is empty, `LlmClient::chat` returns a
//! fabricated response instead of hitting the network. This keeps dev loops
//! and CI free of an external dependency — real keys land later via env.

use std::time::Duration;

use listenai_core::{Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Messages in the OpenAI-compatible shape OpenRouter consumes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Set `Some(true)` to ask the model for a JSON object response.
    pub json_mode: Option<bool>,
    /// Output modalities to request, e.g. `["image", "text"]` for
    /// image-capable models like `google/gemini-2.5-flash-image`. Defaults
    /// to text-only when `None`.
    pub modalities: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    /// First image returned by the model, raw base64 (no `data:` prefix).
    /// Populated only when `modalities` requested image output and the
    /// upstream actually returned one.
    #[serde(default)]
    pub image_base64: Option<String>,
    #[serde(default)]
    pub usage: ChatUsage,
    /// `true` when the response came from the mock path.
    #[serde(default)]
    pub mocked: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// Reported by OpenRouter; present in the wire shape but we bill off
    /// prompt/completion, so keep it for completeness only.
    #[allow(dead_code)]
    pub total_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct LlmClient {
    inner: Client,
    api_key: String,
    base_url: String,
}

impl LlmClient {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        timeout_secs: u64,
    ) -> Result<Self> {
        let inner = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent(concat!("listenai-api/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Other(anyhow::anyhow!("build http client: {e}")))?;
        Ok(Self {
            inner,
            api_key: api_key.into(),
            base_url: base_url.into(),
        })
    }

    /// `true` when no API key is configured — all calls use the mock path.
    pub fn is_mock(&self) -> bool {
        self.api_key.trim().is_empty()
    }

    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        if self.is_mock() {
            return Ok(mock_response(req));
        }
        self.call_openrouter(req).await
    }

    async fn call_openrouter(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut body = json!({
            "model": req.model,
            "messages": req.messages,
        });
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(m) = req.max_tokens {
            body["max_tokens"] = json!(m);
        }
        if req.json_mode == Some(true) {
            body["response_format"] = json!({ "type": "json_object" });
        }
        if let Some(mods) = &req.modalities {
            body["modalities"] = json!(mods);
        }

        let resp = self
            .inner
            .post(&url)
            .bearer_auth(&self.api_key)
            // OpenRouter convention: help them with attribution
            .header("HTTP-Referer", "https://github.com/bartbeecoders/aidbooks")
            .header("X-Title", "ListenAI")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("openrouter: {e}")))?;

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| Error::Upstream(format!("openrouter read: {e}")))?;
        if !status.is_success() {
            let preview = String::from_utf8_lossy(&bytes);
            return Err(Error::Upstream(format!(
                "openrouter returned {status}: {}",
                preview.chars().take(400).collect::<String>()
            )));
        }

        let parsed: Value = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Upstream(format!("openrouter json: {e}")))?;

        let message = parsed
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("message"))
            .ok_or_else(|| {
                Error::Upstream("openrouter: missing choices[0].message".into())
            })?;

        let (content, image_base64) = extract_message(message);

        if content.is_empty() && image_base64.is_none() {
            return Err(Error::Upstream(
                "openrouter: response had neither text nor image".into(),
            ));
        }

        let usage = parsed
            .get("usage")
            .and_then(|u| serde_json::from_value::<ChatUsage>(u.clone()).ok())
            .unwrap_or_default();

        Ok(ChatResponse {
            content,
            image_base64,
            usage,
            mocked: false,
        })
    }
}

/// Pull text + (optional) image out of an OpenRouter `choices[i].message`.
///
/// Image returns vary by model. We accept (in order):
///   1. `message.images[i].image_url.url`           — Gemini image format
///   2. `message.content[i]` array w/ `image_url`   — multi-modal block
///   3. `message.content` plain string              — text-only fallback
///
/// `data:image/...;base64,...` URLs are stripped to the raw base64 payload
/// so callers don't have to re-parse them.
fn extract_message(message: &Value) -> (String, Option<String>) {
    // 1. message.images
    let from_images = message
        .get("images")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter().find_map(|item| {
                item.get("image_url")
                    .and_then(|u| u.get("url"))
                    .and_then(Value::as_str)
                    .map(strip_data_url)
            })
        });

    // 2. message.content (array form)
    let mut text_parts = Vec::<String>::new();
    let mut from_content_image: Option<String> = None;
    if let Some(arr) = message.get("content").and_then(Value::as_array) {
        for block in arr {
            let ty = block.get("type").and_then(Value::as_str).unwrap_or("");
            match ty {
                "text" => {
                    if let Some(t) = block.get("text").and_then(Value::as_str) {
                        text_parts.push(t.to_string());
                    }
                }
                "image_url" => {
                    if from_content_image.is_none() {
                        if let Some(url) = block
                            .get("image_url")
                            .and_then(|u| u.get("url"))
                            .and_then(Value::as_str)
                        {
                            from_content_image = Some(strip_data_url(url));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // 3. message.content (plain string)
    let plain = message
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string);

    let text = if !text_parts.is_empty() {
        text_parts.join("")
    } else {
        plain.unwrap_or_default()
    };

    (text, from_images.or(from_content_image))
}

fn strip_data_url(url: &str) -> String {
    if let Some(idx) = url.find(";base64,") {
        url[(idx + ";base64,".len())..].to_string()
    } else {
        url.to_string()
    }
}

/// Fabricate a plausible response for the mock path. Matches the shapes
/// the generation layer expects for each prompt role.
fn mock_response(req: &ChatRequest) -> ChatResponse {
    // Look at the last user message to decide which role we're mocking.
    let last_user = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("");

    let wants_image = req
        .modalities
        .as_ref()
        .map(|m| m.iter().any(|s| s == "image"))
        .unwrap_or(false);

    let (content, image_base64) = if wants_image {
        ("Mock cover art.".to_string(), Some(mock_cover_png_base64()))
    } else if req.json_mode == Some(true) && last_user.contains("audiobook outline") {
        (mock_outline(last_user), None)
    } else if req.json_mode == Some(true) && last_user.contains("audiobook topic") {
        (mock_random_topic(last_user), None)
    } else {
        (mock_chapter(last_user), None)
    };

    // Rough token estimates for the mock path.
    let prompt_tokens = (req.messages.iter().map(|m| m.content.len()).sum::<usize>() / 4) as u32;
    let completion_tokens = (content.len() / 4) as u32;

    ChatResponse {
        content,
        image_base64,
        usage: ChatUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
        mocked: true,
    }
}

/// Single-pixel transparent PNG. Just enough that the mock path returns
/// well-formed image bytes without bundling an asset.
fn mock_cover_png_base64() -> String {
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=".to_string()
}

fn mock_outline(prompt: &str) -> String {
    // Pull chapter count out of the prompt if we can ("{chapter_count}" already
    // substituted). Default to 3.
    let count = find_number_after(prompt, "Length preset:").unwrap_or(3);
    let topic =
        take_phrase_after(prompt, "Topic:").unwrap_or_else(|| "an unnamed topic".to_string());
    let mut chapters = Vec::new();
    for n in 1..=count {
        chapters.push(json!({
            "number": n,
            "title": format!("Chapter {n}"),
            "synopsis": format!("Mock content covering aspect {n} of {topic}."),
            "target_words": 500,
        }));
    }
    serde_json::to_string(&json!({
        "title": format!("A Short Listen About {topic}"),
        "subtitle": "",
        "chapters": chapters,
    }))
    .unwrap_or_else(|_| "{}".into())
}

fn mock_chapter(prompt: &str) -> String {
    let title = take_phrase_after(prompt, "Chapter").unwrap_or_else(|| "an unnamed chapter".into());
    format!(
        "This is a mock chapter. It exists so development can proceed without a real \
         OpenRouter API key. The chapter is titled {title} and would, in production, \
         contain around the target word count of flowing prose.\n\n\
         Additional paragraphs of mock content follow. They are short on purpose so tests \
         run fast. Once a real key is configured, actual model output replaces this."
    )
}

fn mock_random_topic(_prompt: &str) -> String {
    serde_json::to_string(&json!({
        "topic": "The hidden history of the telegraph key and the first global network",
        "genre": "history",
        "length": "short",
    }))
    .unwrap_or_else(|_| "{}".into())
}

fn find_number_after(haystack: &str, needle: &str) -> Option<u32> {
    let start = haystack.find(needle)? + needle.len();
    let window = &haystack[start..];
    let mut digits = String::new();
    for c in window.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else if !digits.is_empty() {
            break;
        }
    }
    digits.parse().ok()
}

fn take_phrase_after(haystack: &str, needle: &str) -> Option<String> {
    let start = haystack.find(needle)? + needle.len();
    let rest = &haystack[start..];
    let line = rest.lines().next()?.trim().trim_end_matches(':').trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_mode_outline_is_valid_json() {
        let c = LlmClient::new("", "http://unused", 5).unwrap();
        let resp = c
            .chat(&ChatRequest {
                model: "mock".into(),
                messages: vec![
                    ChatMessage::system("sys"),
                    ChatMessage::user("Build an audiobook outline. Topic: space exploration\nLength preset: medium 6 chapters"),
                ],
                temperature: Some(0.5),
                max_tokens: Some(800),
                json_mode: Some(true),
                modalities: None,
            })
            .await
            .unwrap();
        assert!(resp.mocked);
        let v: Value = serde_json::from_str(&resp.content).unwrap();
        assert_eq!(v["chapters"].as_array().unwrap().len(), 6);
    }

    #[tokio::test]
    async fn mock_mode_chapter_is_plain_prose() {
        let c = LlmClient::new("", "http://unused", 5).unwrap();
        let resp = c
            .chat(&ChatRequest {
                model: "mock".into(),
                messages: vec![ChatMessage::user(
                    "Chapter 1: the beginning\nWrite chapter prose.",
                )],
                temperature: None,
                max_tokens: None,
                json_mode: None,
                modalities: None,
            })
            .await
            .unwrap();
        assert!(resp.mocked);
        assert!(resp.content.starts_with("This is a mock chapter"));
    }
}
