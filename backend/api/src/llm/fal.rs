//! HTTP client for [fal.ai](https://fal.ai/docs/documentation) hosted
//! image generation.
//!
//! fal exposes two REST hosts:
//!   * `https://fal.run/{model_id}` — synchronous blocking POST, returns
//!     the result in the same response.
//!   * `https://queue.fal.run/{model_id}` — async submit + poll pattern.
//!
//! We use the sync host so cover/chapter art generation stays a single
//! awaited call (matching the OpenRouter / xAI / mold paths). To avoid
//! a follow-up download for the image URL the request body sets
//! `sync_mode: true`, which tells fal to inline each generated image as
//! a `data:image/...;base64,...` URI inside `images[i].url`. We strip
//! the data-URL prefix and decode the bytes inline.
//!
//! Auth: `Authorization: Key <FAL_KEY>` (not Bearer — fal uses its own
//! `Key` scheme). API keys live on the LLM row (`api_key_enc`).

use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use listenai_core::{Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Default host when an admin leaves `base_url` blank on a fal row.
/// The sync API uses the apex domain `fal.run`; the queue API uses
/// `queue.fal.run`. We always prefer sync so we don't have to poll.
pub const DEFAULT_FAL_BASE_URL: &str = "https://fal.run";

/// Per-image request to a fal model endpoint. `prompt` is required; the
/// other fields cover the FLUX-family parameters (the most common image
/// models). `sync_mode = true` makes fal embed the image as a data URI in
/// the response so we don't need a follow-up GET on the URL.
#[derive(Debug, Clone, Serialize)]
pub struct FalRequest {
    pub prompt: String,
    /// e.g. `square_hd`, `portrait_16_9`, or an explicit `{width, height}`
    /// object. We send a string preset so common aspects stay legible in
    /// the request log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_inference_steps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance_scale: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_images: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Embed images as data URIs in the response. Required for our
    /// single-shot sync flow — otherwise `url` is a presigned R2 link we'd
    /// have to fetch separately.
    pub sync_mode: bool,
}

/// What fal returned. We surface the first image's raw bytes plus the
/// reported MIME type and seed so the generation log keeps a useful
/// reproduction signal.
#[derive(Debug, Clone)]
pub struct FalResponse {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub seed_used: Option<i64>,
}

/// Send a generate request to fal and return the raw image bytes +
/// metadata. `model` is the fal slug (e.g. `fal-ai/flux/dev`); `base_url`
/// is the sync host (defaults to [`DEFAULT_FAL_BASE_URL`]).
pub async fn generate(
    base_url: Option<&str>,
    api_key: &str,
    model: &str,
    req: &FalRequest,
) -> Result<FalResponse> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(Error::Validation(
            "fal provider requires `api_key` (FAL_KEY) on the llm row".into(),
        ));
    }
    let base = base_url
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_FAL_BASE_URL)
        .trim_end_matches('/');
    let slug = model.trim().trim_start_matches('/');
    if slug.is_empty() {
        return Err(Error::Validation(
            "fal provider requires a non-empty `model_id` (e.g. fal-ai/flux/dev)".into(),
        ));
    }
    let url = format!("{base}/{slug}");

    let timeout = std::env::var("FAL_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(300);
    let client = Client::builder()
        .timeout(Duration::from_secs(timeout))
        .user_agent(concat!("listenai-api-fal/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Other(anyhow::anyhow!("build fal http client: {e}")))?;

    let resp = client
        .post(&url)
        // fal uses `Authorization: Key <FAL_KEY>` (not Bearer). reqwest's
        // header API lets us send the raw scheme.
        .header(reqwest::header::AUTHORIZATION, format!("Key {key}"))
        .json(req)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("fal generate: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("fal generate read: {e}")))?;

    if !status.is_success() {
        return Err(Error::Upstream(format_fal_error(status, &bytes)));
    }

    let envelope: FalEnvelope = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Upstream(format!("fal generate json: {e}")))?;
    let first = envelope
        .images
        .into_iter()
        .next()
        .ok_or_else(|| Error::Upstream("fal generate: empty images[]".into()))?;
    let (img_bytes, content_type) = decode_data_url(&first.url)?;
    Ok(FalResponse {
        bytes: img_bytes,
        content_type: first
            .content_type
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(content_type),
        seed_used: envelope.seed.map(|s| s as i64),
    })
}

/// Map the `is_short` cover flag to a fal `image_size` preset. FLUX-family
/// models accept the named presets `square_hd` (1024×1024) and
/// `portrait_16_9` (768×1366 ≈ 9:16) which match the rest of the cover
/// pipeline's aspect targets.
pub fn image_size_for(is_short: bool) -> &'static str {
    if is_short {
        "portrait_16_9"
    } else {
        "square_hd"
    }
}

/// fal.ai's sync-mode response envelope (subset). All models in the
/// FLUX / Ideogram / SDXL families ship the same shape: an `images`
/// array plus optional `seed` and `has_nsfw_concepts`.
#[derive(Debug, Deserialize)]
struct FalEnvelope {
    #[serde(default)]
    images: Vec<FalImage>,
    #[serde(default)]
    seed: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct FalImage {
    /// In sync mode this is a `data:image/...;base64,...` URI; otherwise
    /// a presigned R2 URL we'd have to GET separately.
    url: String,
    #[serde(default)]
    content_type: Option<String>,
}

/// Pull bytes + MIME out of a `data:image/...;base64,...` URI. Returns
/// an Upstream error when the URL is missing the data-URL prefix (which
/// happens if `sync_mode` was dropped from the request) — far clearer
/// than a generic base64 decode failure.
fn decode_data_url(url: &str) -> Result<(Vec<u8>, String)> {
    let rest = url.strip_prefix("data:").ok_or_else(|| {
        Error::Upstream(
            "fal: image URL is not a data: URI — make sure `sync_mode: true` \
             is set on the request"
                .into(),
        )
    })?;
    let (meta, b64) = rest.split_once(";base64,").ok_or_else(|| {
        Error::Upstream("fal: data URI is missing the `;base64,` segment".into())
    })?;
    let mime = if meta.is_empty() {
        "image/png".to_string()
    } else {
        meta.to_string()
    };
    let bytes = B64
        .decode(b64.as_bytes())
        .map_err(|e| Error::Upstream(format!("fal: decode image base64: {e}")))?;
    if bytes.is_empty() {
        return Err(Error::Upstream("fal: empty image payload".into()));
    }
    Ok((bytes, mime))
}

/// Format a fal upstream failure for the admin. fal returns a JSON error
/// envelope shaped roughly like `{ "detail": "..." }` or `{ "detail": [
/// {"loc": [...], "msg": "..."} ] }` (FastAPI style). We try both, then
/// fall back to a truncated body preview.
fn format_fal_error(status: reqwest::StatusCode, body: &[u8]) -> String {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(s) = value.get("detail").and_then(|v| v.as_str()) {
            return format!("fal {status}: {s}");
        }
        if let Some(arr) = value.get("detail").and_then(|v| v.as_array()) {
            let msgs: Vec<String> = arr
                .iter()
                .filter_map(|item| {
                    item.get("msg")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            if !msgs.is_empty() {
                return format!("fal {status}: {}", msgs.join("; "));
            }
        }
    }
    let preview = String::from_utf8_lossy(body);
    format!(
        "fal generate {status}: {}",
        preview.chars().take(400).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_data_url_parses_mime_and_bytes() {
        // PNG header bytes "iVBOR..." → "\x89PNG\r\n..."
        let url = "data:image/png;base64,iVBORw0KGgo=";
        let (bytes, mime) = decode_data_url(url).unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(&bytes[..4], b"\x89PNG");
    }

    #[test]
    fn decode_data_url_rejects_plain_url() {
        let err = decode_data_url("https://fal.media/files/abc.png").unwrap_err();
        assert!(err.to_string().contains("sync_mode"));
    }
}
