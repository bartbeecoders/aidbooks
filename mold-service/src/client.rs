//! Thin HTTP client for the upstream `mold serve` instance. Mirrors
//! the wire shape that mold exposes (`/api/generate`, `/api/models/...`)
//! without leaking it through this service's public API.

use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct UpstreamGenerateRequest {
    pub prompt: String,
    pub model: String,
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    pub output_format: String,
}

#[derive(Debug, Clone)]
pub struct UpstreamResponse {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub seed_used: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MoldErrorEnvelope {
    error: String,
    #[serde(default)]
    code: Option<String>,
}

fn build_client(timeout: Duration) -> Result<Client> {
    Client::builder()
        .timeout(timeout)
        .user_agent(concat!("mold-service/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Internal(anyhow::anyhow!("build upstream client: {e}")))
}

pub async fn generate(
    base_url: &str,
    api_key: Option<&str>,
    timeout_secs: u64,
    req: &UpstreamGenerateRequest,
) -> Result<UpstreamResponse> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/generate");
    let client = build_client(Duration::from_secs(timeout_secs))?;
    let mut builder = client.post(&url).json(req);
    if let Some(key) = api_key.map(str::trim).filter(|s| !s.is_empty()) {
        builder = builder.header("X-Api-Key", key);
    }

    let resp = builder
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("mold generate: {e}")))?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("mold generate read: {e}")))?;

    if !status.is_success() {
        return Err(Error::Upstream(format_mold_error(status, &bytes)));
    }
    if bytes.is_empty() {
        return Err(Error::Upstream("mold generate: empty payload".into()));
    }
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/png")
        .to_string();
    let seed_used = headers
        .get("x-mold-seed-used")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok());

    Ok(UpstreamResponse {
        bytes: bytes.to_vec(),
        content_type,
        seed_used,
    })
}

pub async fn pull(
    base_url: &str,
    api_key: Option<&str>,
    timeout_secs: u64,
    model: &str,
) -> Result<String> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/models/pull");
    let client = build_client(Duration::from_secs(timeout_secs))?;
    let body = serde_json::json!({ "model": model });
    let mut builder = client.post(&url).json(&body);
    if let Some(key) = api_key.map(str::trim).filter(|s| !s.is_empty()) {
        builder = builder.header("X-Api-Key", key);
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("mold pull: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("mold pull read: {e}")))?;
    if !status.is_success() {
        return Err(Error::Upstream(format_mold_error(status, &bytes)));
    }
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

pub async fn unload(base_url: &str, api_key: Option<&str>) -> Result<String> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/models/unload");
    // Unload should be near-instant; cap it short so a wedged mold
    // server doesn't make the admin button hang for minutes.
    let client = build_client(Duration::from_secs(30))?;
    let mut builder = client.delete(&url);
    if let Some(key) = api_key.map(str::trim).filter(|s| !s.is_empty()) {
        builder = builder.header("X-Api-Key", key);
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("mold unload: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("mold unload read: {e}")))?;
    if !status.is_success() {
        return Err(Error::Upstream(format_mold_error(status, &bytes)));
    }
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// Best-effort liveness probe against mold serve's `/healthz`. Returns
/// `false` on any failure so `/healthz` on this service can still
/// answer `200 OK` (with `upstream_reachable: false`) even when mold
/// is down. The mold-service is up if its own HTTP listener is up.
pub async fn health(base_url: &str) -> bool {
    let Ok(client) = build_client(Duration::from_secs(5)) else {
        return false;
    };
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/healthz");
    matches!(
        client.get(&url).send().await,
        Ok(r) if r.status().is_success()
    )
}

fn format_mold_error(status: StatusCode, body: &[u8]) -> String {
    if let Ok(env) = serde_json::from_slice::<MoldErrorEnvelope>(body) {
        return match env.code.as_deref() {
            Some(code) if !code.is_empty() => format!("mold {status} [{code}]: {}", env.error),
            _ => format!("mold {status}: {}", env.error),
        };
    }
    let preview = String::from_utf8_lossy(body);
    format!(
        "mold {status}: {}",
        preview.chars().take(400).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_json_error_with_code() {
        let body = br#"{"error":"model not found","code":"MODEL_NOT_FOUND"}"#;
        let msg = format_mold_error(StatusCode::NOT_FOUND, body);
        assert!(msg.contains("MODEL_NOT_FOUND"));
        assert!(msg.contains("model not found"));
    }

    #[test]
    fn formats_plain_error_fallback() {
        let body = b"<html>nginx 502</html>";
        let msg = format_mold_error(StatusCode::BAD_GATEWAY, body);
        assert!(msg.contains("502"));
        assert!(msg.contains("nginx"));
    }
}
