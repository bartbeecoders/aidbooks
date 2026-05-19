//! HTTP client for the project-local `mold-service` (see
//! `/mold-service`). The service wraps the upstream
//! [`mold serve`](https://github.com/utensils/mold) HTTP API and owns
//! the AidBooks-flavored policy that used to live here: the GPU
//! semaphore, the OOM cooldown, the default model / steps / guidance,
//! and the 9:16 shorts vs square dimensions.
//!
//! From the backend's perspective the LLM row's `base_url` now points
//! at the mold-service URL (e.g. `http://127.0.0.1:7681`), not at
//! `mold serve` directly. The decision to keep `MoldRequest` /
//! `MoldResponse` shapes here means callers in `cover.rs` and
//! `handlers/admin.rs` don't need to learn a second protocol.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use listenai_core::{Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Per-image request to mold-service. Only `prompt` is required; the
/// service fills in `model`, dimensions, steps, and guidance from its
/// own policy when fields are left `None`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct MoldRequest {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When `true` (and `width`/`height` are unset), mold-service
    /// picks a 9:16 portrait. Default `false` → square.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_short: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    /// `png` (default), `jpeg`, `webp`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
}

/// What mold-service returned. `bytes` is the decoded image payload —
/// callers re-encode to base64 for the ChatResponse path. `width` /
/// `height` reflect the values mold-service actually used (after
/// applying its defaults), so cost-per-megapixel logic stays correct
/// when the request omitted explicit dimensions.
#[derive(Debug, Clone)]
pub struct MoldResponse {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub width: u32,
    pub height: u32,
    pub model: String,
    pub steps: u32,
    pub seed_used: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct GenerateResponseBody {
    image_base64: String,
    content_type: String,
    width: u32,
    height: u32,
    model: String,
    steps: u32,
    #[serde(default)]
    seed_used: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ServiceErrorBody {
    error: String,
}

#[derive(Debug, Serialize)]
struct PullBody<'a> {
    model: &'a str,
}

#[derive(Debug, Deserialize)]
struct MessageBody {
    message: String,
}

fn build_client(timeout: Duration) -> Result<Client> {
    Client::builder()
        .timeout(timeout)
        .user_agent(concat!("listenai-api-mold/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Other(anyhow::anyhow!("build mold-service client: {e}")))
}

fn generate_timeout() -> Duration {
    // Mirror the upstream default: 300s lets a cold model load through.
    // `MOLD_TIMEOUT_SECS` lives in mold-service now, but keep an env
    // override here too so the backend can bound its own waits
    // independently if mold-service is slow to respond.
    let secs = std::env::var("MOLD_SERVICE_TIMEOUT_SECS")
        .or_else(|_| std::env::var("MOLD_TIMEOUT_SECS"))
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(300);
    Duration::from_secs(secs)
}

fn pull_timeout() -> Duration {
    let secs = std::env::var("MOLD_SERVICE_PULL_TIMEOUT_SECS")
        .or_else(|_| std::env::var("MOLD_PULL_TIMEOUT_SECS"))
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(3600);
    Duration::from_secs(secs)
}

/// Send a generate request and return the decoded image bytes +
/// metadata. The mold-service owns concurrency and OOM cooldown; the
/// backend just needs to wait for the response.
pub async fn generate(
    base_url: &str,
    api_key: Option<&str>,
    req: &MoldRequest,
) -> Result<MoldResponse> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/v1/generate");

    let client = build_client(generate_timeout())?;
    let mut builder = client.post(&url).json(req);
    if let Some(key) = api_key.map(str::trim).filter(|s| !s.is_empty()) {
        builder = builder.header("X-Api-Key", key);
    }

    let resp = builder
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("mold-service generate: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("mold-service generate read: {e}")))?;
    if !status.is_success() {
        return Err(Error::Upstream(format_service_error(status, &bytes)));
    }

    let body: GenerateResponseBody = serde_json::from_slice(&bytes).map_err(|e| {
        Error::Upstream(format!(
            "mold-service generate: invalid JSON response: {e}"
        ))
    })?;
    let decoded = B64.decode(body.image_base64.as_bytes()).map_err(|e| {
        Error::Upstream(format!("mold-service generate: bad image_base64: {e}"))
    })?;
    if decoded.is_empty() {
        return Err(Error::Upstream(
            "mold-service generate: empty image payload".into(),
        ));
    }

    Ok(MoldResponse {
        bytes: decoded,
        content_type: body.content_type,
        width: body.width,
        height: body.height,
        model: body.model,
        steps: body.steps,
        seed_used: body.seed_used,
    })
}

/// Pull a model on the upstream mold instance via mold-service. Blocks
/// until the download finishes — large families can take many minutes,
/// so the default timeout is 60 minutes (override with
/// `MOLD_SERVICE_PULL_TIMEOUT_SECS`).
pub async fn pull(base_url: &str, api_key: Option<&str>, model: &str) -> Result<String> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/v1/models/pull");

    let client = build_client(pull_timeout())?;
    let mut builder = client.post(&url).json(&PullBody { model });
    if let Some(key) = api_key.map(str::trim).filter(|s| !s.is_empty()) {
        builder = builder.header("X-Api-Key", key);
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("mold-service pull: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("mold-service pull read: {e}")))?;
    if !status.is_success() {
        return Err(Error::Upstream(format_service_error(status, &bytes)));
    }
    let body: MessageBody = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Upstream(format!("mold-service pull: invalid JSON: {e}")))?;
    Ok(body.message)
}

/// Drop every loaded model from the upstream mold's GPU cache via
/// mold-service. Server-wide.
pub async fn unload(base_url: &str, api_key: Option<&str>) -> Result<String> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/v1/models/unload");

    // Unload should be near-instant; cap it short so a wedged service
    // doesn't make the admin button hang for minutes.
    let client = build_client(Duration::from_secs(30))?;
    let mut builder = client.delete(&url);
    if let Some(key) = api_key.map(str::trim).filter(|s| !s.is_empty()) {
        builder = builder.header("X-Api-Key", key);
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("mold-service unload: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("mold-service unload read: {e}")))?;
    if !status.is_success() {
        return Err(Error::Upstream(format_service_error(status, &bytes)));
    }
    let body: MessageBody = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Upstream(format!("mold-service unload: invalid JSON: {e}")))?;
    Ok(body.message)
}

/// Convenience: turn the response bytes into a base64 string for the
/// existing `ChatResponse.image_base64` shape so the generation_event
/// path stays identical to the OpenRouter/xAI image flow.
pub fn bytes_to_b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// Format a mold-service failure as a single-line message. Falls back
/// to a truncated body preview on parse failure so the admin still
/// sees *something* useful when the response isn't well-formed JSON
/// (e.g. an upstream proxy 502 page).
fn format_service_error(status: reqwest::StatusCode, body: &[u8]) -> String {
    if let Ok(env) = serde_json::from_slice::<ServiceErrorBody>(body) {
        return format!("mold-service {status}: {}", env.error);
    }
    let preview = String::from_utf8_lossy(body);
    format!(
        "mold-service {status}: {}",
        preview.chars().take(400).collect::<String>()
    )
}
