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
    /// Which upstream to dispatch this request to. `None` or
    /// `Some("open_router")` = OpenRouter; `Some("xai")` = native xAI host.
    /// Skipped on the wire — controls routing only.
    #[serde(skip_serializing)]
    pub provider: Option<String>,
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
    #[serde(default)]
    pub total_tokens: u32,
    /// Actual USD cost of this request as billed by OpenRouter — populated
    /// when `usage: { include: true }` was sent in the request body. `0.0`
    /// for free / BYOK models, the mock path, and any provider that doesn't
    /// report a cost. We persist this directly into `generation_event`.
    #[serde(default)]
    pub cost: f64,
}

#[derive(Debug, Clone)]
pub struct LlmClient {
    inner: Client,
    /// OpenRouter credentials.
    api_key: String,
    base_url: String,
    /// xAI credentials. Empty key → xAI calls fall back to mock just like
    /// OpenRouter does. We always store both so a single client can route
    /// per-row by provider.
    xai_api_key: String,
    xai_base_url: String,
}

impl LlmClient {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        xai_api_key: impl Into<String>,
        xai_base_url: impl Into<String>,
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
            xai_api_key: xai_api_key.into(),
            xai_base_url: xai_base_url.into(),
        })
    }

    /// `true` when no OpenRouter API key is configured — calls dispatched
    /// to OpenRouter (the default provider) use the mock path.
    pub fn is_mock(&self) -> bool {
        self.api_key.trim().is_empty()
    }

    /// Whether xAI calls are mocked (empty key).
    pub fn is_xai_mock(&self) -> bool {
        self.xai_api_key.trim().is_empty()
    }

    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        match req.provider.as_deref() {
            Some("xai") => {
                let mock = self.is_xai_mock();
                tracing::info!(
                    provider = "xai",
                    model = %req.model,
                    mock,
                    "llm chat dispatch"
                );
                if mock {
                    return Ok(mock_response(req));
                }
                self.call_chat(req, &self.xai_base_url, &self.xai_api_key, false)
                    .await
            }
            // None or `Some("open_router")` — anything else falls through to
            // the legacy OpenRouter path so unknown values fail loudly there.
            other => {
                let mock = self.is_mock();
                let provider_label = other.unwrap_or("open_router");
                tracing::info!(
                    provider = provider_label,
                    model = %req.model,
                    mock,
                    "llm chat dispatch"
                );
                if mock {
                    return Ok(mock_response(req));
                }
                self.call_chat(req, &self.base_url, &self.api_key, true)
                    .await
            }
        }
    }

    /// List the OpenRouter model catalog. The endpoint is public — no API
    /// key needed — so this works in mock mode too.
    ///
    /// `output_modalities` is forwarded as a query param. OpenRouter's
    /// unfiltered `/models` only returns ~7 image-output models because it
    /// prefers chat-shaped rows; passing `Some("image")` here surfaces the
    /// full image-generation catalog (Sourceful, FLUX, ByteDance, …).
    pub async fn list_openrouter_models(
        &self,
        output_modalities: Option<&str>,
    ) -> Result<Vec<OpenRouterModel>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let mut req = self.inner.get(&url);
        if let Some(om) = output_modalities {
            let trimmed = om.trim();
            if !trimmed.is_empty() {
                req = req.query(&[("output_modalities", trimmed)]);
            }
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("openrouter models: {e}")))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| Error::Upstream(format!("openrouter models read: {e}")))?;
        if !status.is_success() {
            let preview = String::from_utf8_lossy(&bytes);
            return Err(Error::Upstream(format!(
                "openrouter models {status}: {}",
                preview.chars().take(400).collect::<String>()
            )));
        }
        #[derive(Deserialize)]
        struct Envelope {
            data: Vec<OpenRouterModel>,
        }
        let env: Envelope = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Upstream(format!("openrouter models json: {e}")))?;
        Ok(env.data)
    }

    /// List xAI's `language-models` catalog. xAI requires a Bearer token
    /// even for the catalog (unlike OpenRouter), so this errors when no
    /// xAI key is configured.
    pub async fn list_xai_models(&self) -> Result<Vec<XaiLanguageModel>> {
        if self.is_xai_mock() {
            return Err(Error::Validation(
                "xAI model catalog requires xai_api_key to be configured".into(),
            ));
        }
        let url = format!(
            "{}/language-models",
            self.xai_base_url.trim_end_matches('/')
        );
        let resp = self
            .inner
            .get(&url)
            .bearer_auth(&self.xai_api_key)
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("xai models: {e}")))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| Error::Upstream(format!("xai models read: {e}")))?;
        if !status.is_success() {
            let preview = String::from_utf8_lossy(&bytes);
            return Err(Error::Upstream(format!(
                "xai models {status}: {}",
                preview.chars().take(400).collect::<String>()
            )));
        }
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(default)]
            models: Vec<XaiLanguageModel>,
        }
        let env: Envelope = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Upstream(format!("xai models json: {e}")))?;
        Ok(env.models)
    }

    /// List xAI's `image-generation-models` catalog. Same auth + envelope
    /// shape as `/language-models`, just a different model class.
    pub async fn list_xai_image_models(&self) -> Result<Vec<XaiImageModel>> {
        if self.is_xai_mock() {
            return Err(Error::Validation(
                "xAI image catalog requires xai_api_key to be configured".into(),
            ));
        }
        let url = format!(
            "{}/image-generation-models",
            self.xai_base_url.trim_end_matches('/')
        );
        let resp = self
            .inner
            .get(&url)
            .bearer_auth(&self.xai_api_key)
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("xai image models: {e}")))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| Error::Upstream(format!("xai image models read: {e}")))?;
        if !status.is_success() {
            let preview = String::from_utf8_lossy(&bytes);
            return Err(Error::Upstream(format!(
                "xai image models {status}: {}",
                preview.chars().take(400).collect::<String>()
            )));
        }
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(default)]
            models: Vec<XaiImageModel>,
        }
        let env: Envelope = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Upstream(format!("xai image models json: {e}")))?;
        Ok(env.models)
    }

    /// Generate one image via xAI's `/images/generations` endpoint
    /// (separate from chat completions — different request shape).
    /// Returns the raw base64 payload (no `data:` prefix).
    pub async fn generate_xai_image(&self, model: &str, prompt: &str) -> Result<String> {
        if self.is_xai_mock() {
            // Match the chat-mock contract: a 1×1 transparent PNG.
            return Ok(mock_cover_png_base64());
        }
        let url = format!(
            "{}/images/generations",
            self.xai_base_url.trim_end_matches('/')
        );
        let body = json!({
            "model": model,
            "prompt": prompt,
            "n": 1,
            "response_format": "b64_json",
        });
        let resp = self
            .inner
            .post(&url)
            .bearer_auth(&self.xai_api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("xai image gen: {e}")))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| Error::Upstream(format!("xai image gen read: {e}")))?;
        if !status.is_success() {
            let preview = String::from_utf8_lossy(&bytes);
            return Err(Error::Upstream(format!(
                "xai image gen {status}: {}",
                preview.chars().take(400).collect::<String>()
            )));
        }
        let parsed: Value = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Upstream(format!("xai image gen json: {e}")))?;
        let b64 = parsed
            .get("data")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|item| item.get("b64_json"))
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Upstream("xai image gen: missing data[0].b64_json".into()))?;
        Ok(b64.to_string())
    }
}

/// Subset of xAI's `/language-models` response that we surface to admins.
/// All fields are optional in case the API drops/renames pieces — see the
/// same defensive default we apply to OpenRouter.
///
/// xAI publishes prices as integer **microdollars per million tokens**:
/// e.g. `prompt_text_token_price: 3000000` ≈ $3.00 per 1M prompt tokens.
/// To match our `cost_*_per_1k` columns, divide by 1_000_000_000.
#[derive(Debug, Clone, Deserialize)]
pub struct XaiLanguageModel {
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub output_modalities: Vec<String>,
    #[serde(default)]
    pub prompt_text_token_price: Option<u64>,
    #[serde(default)]
    pub completion_text_token_price: Option<u64>,
    /// Window size in tokens, when reported. Newer xAI models include
    /// this; older ones don't.
    #[serde(default)]
    pub max_prompt_length: Option<u64>,
}

/// Subset of xAI's `/image-generation-models` response.
///
/// Pricing: xAI returns `image_generation_price` as integer microdollars
/// per generated image (so `70_000` ≈ $0.07/image). To match our
/// `cost_per_megapixel` admin column — which we already use as a $/image
/// proxy for image-priced models — divide by 1_000_000.
#[derive(Debug, Clone, Deserialize)]
pub struct XaiImageModel {
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub output_modalities: Vec<String>,
    /// Per-generated-image price in microdollars. Optional so a missing
    /// or renamed upstream field doesn't poison the catalog fetch.
    #[serde(default)]
    pub image_generation_price: Option<u64>,
    #[serde(default)]
    pub max_prompt_length: Option<u64>,
}

/// Subset of OpenRouter's `/models` response that we surface to admins.
/// All upstream fields the picker needs are optional — older / newer models
/// occasionally drop or rename pieces, and we'd rather lose pricing on one
/// row than fail the whole catalog fetch.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenRouterModel {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub context_length: Option<u64>,
    #[serde(default)]
    pub architecture: Option<OpenRouterArchitecture>,
    #[serde(default)]
    pub pricing: Option<OpenRouterPricing>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenRouterArchitecture {
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub output_modalities: Vec<String>,
}

/// Prices are shipped as decimal *strings* keyed in USD-per-token (or
/// per-image for `image`). Parse on the consumer side so a malformed value
/// doesn't poison the whole row.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenRouterPricing {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub completion: Option<String>,
    /// $ per generated image (only set on image-output models).
    #[serde(default)]
    pub image: Option<String>,
}

impl LlmClient {
    /// Generic chat-completions caller. OpenRouter and xAI share the
    /// OpenAI-compatible wire shape; only host, key, and a couple of
    /// attribution headers differ. `add_or_attribution` is true only for
    /// the OpenRouter path.
    async fn call_chat(
        &self,
        req: &ChatRequest,
        base_url: &str,
        api_key: &str,
        add_or_attribution: bool,
    ) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
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
        // Ask OpenRouter to include the actual billed cost in the usage
        // block. Without this, `usage.cost` is omitted; with it, we get a
        // USD float that drives the per-audiobook cost UI.
        body["usage"] = json!({ "include": true });

        let mut req_builder = self.inner.post(&url).bearer_auth(api_key);
        if add_or_attribution {
            // OpenRouter convention: help them with attribution.
            req_builder = req_builder
                .header("HTTP-Referer", "https://github.com/bartbeecoders/aidbooks")
                .header("X-Title", "ListenAI");
        }
        let resp = req_builder
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("chat: {e}")))?;

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

        let choice = parsed
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .ok_or_else(|| Error::Upstream("openrouter: missing choices[0]".into()))?;
        let message = choice
            .get("message")
            .ok_or_else(|| Error::Upstream("openrouter: missing choices[0].message".into()))?;

        let (content, image_base64) = extract_message(message);

        if content.is_empty() && image_base64.is_none() {
            // Empty / structured-but-empty response. Common causes:
            //   (1) Content-filter refusal — Gemini image-gen flips to
            //       `finish_reason: "PROHIBITED_CONTENT"` with empty
            //       content+images when a chapter excerpt brushes its
            //       safety policy.
            //   (2) `finish_reason: "length"` — `max_tokens` was hit
            //       before the model emitted the image payload.
            //   (3) Transient upstream glitch — retry succeeds.
            // Log the body + finish reason so we can tell them apart,
            // and surface whichever signal upstream gave us.
            let reason = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .or_else(|| choice.get("native_finish_reason").and_then(Value::as_str));
            let refusal = message.get("refusal").and_then(Value::as_str);
            let upstream_error = parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str);

            let preview = serde_json::to_string(&parsed)
                .unwrap_or_default()
                .chars()
                .take(600)
                .collect::<String>();
            tracing::warn!(
                model = %req.model,
                finish_reason = ?reason,
                refusal = ?refusal,
                upstream_error = ?upstream_error,
                body = %preview,
                "openrouter: empty response payload",
            );

            let detail = match (reason, refusal, upstream_error) {
                (_, Some(r), _) if !r.is_empty() => format!("model refused: {r}"),
                (Some(r), _, Some(e)) => format!("finish_reason={r}, error={e}"),
                (Some(r), _, _) => format!("finish_reason={r}"),
                (_, _, Some(e)) => format!("upstream error: {e}"),
                _ => "no text or image returned".into(),
            };
            return Err(Error::Upstream(format!("openrouter: {detail}")));
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
                "image_url" if from_content_image.is_none() => {
                    if let Some(url) = block
                        .get("image_url")
                        .and_then(|u| u.get("url"))
                        .and_then(Value::as_str)
                    {
                        from_content_image = Some(strip_data_url(url));
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
            cost: 0.0,
        },
        mocked: true,
    }
}

/// Single-pixel transparent PNG. Just enough that the mock path returns
/// well-formed image bytes without bundling an asset.
fn mock_cover_png_base64() -> String {
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII="
        .to_string()
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
        // Sample X.ai TTS speech-tag palette so dev runs without a real
        // outline LLM still exercise the inline-tag path through to TTS.
        "tags": ["[pause]", "<soft>", "<slow>"],
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
        let c = LlmClient::new("", "http://unused", "", "http://unused", 5).unwrap();
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
                provider: None,
            })
            .await
            .unwrap();
        assert!(resp.mocked);
        let v: Value = serde_json::from_str(&resp.content).unwrap();
        assert_eq!(v["chapters"].as_array().unwrap().len(), 6);
    }

    #[tokio::test]
    async fn mock_mode_chapter_is_plain_prose() {
        let c = LlmClient::new("", "http://unused", "", "http://unused", 5).unwrap();
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
                provider: None,
            })
            .await
            .unwrap();
        assert!(resp.mocked);
        assert!(resp.content.starts_with("This is a mock chapter"));
    }
}
