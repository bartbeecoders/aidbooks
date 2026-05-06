//! Thin HTTP proxy to the listenai-api.

use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ApiClient {
    base_url: String,
    inner: reqwest::Client,
    default_token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("invalid url: {0}")]
    Url(String),
    #[error("api {status}: {body}")]
    Status { status: u16, body: String },
    #[error("decode: {0}")]
    Decode(String),
}

#[derive(Debug)]
pub struct ApiResponse {
    pub status: u16,
    pub body: Value,
}

impl ApiClient {
    pub fn new(
        base_url: &str,
        default_token: Option<String>,
        timeout_secs: u64,
    ) -> Result<Self, ApiError> {
        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            inner,
            default_token,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Resolve the bearer token for a call: explicit `_token` arg wins, then
    /// the configured default (env), then no auth.
    pub fn resolve_token(&self, explicit: Option<&str>) -> Option<String> {
        explicit
            .map(str::to_string)
            .or_else(|| self.default_token.clone())
    }

    /// Perform a JSON-in / JSON-out call. `body` of `Value::Null` skips the
    /// body. Non-2xx still resolves to `ApiResponse` so the agent gets a
    /// meaningful error payload back.
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
        token: Option<&str>,
    ) -> Result<ApiResponse, ApiError> {
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        let url = format!("{}{}", self.base_url, path);
        let mut url = reqwest::Url::parse(&url).map_err(|e| ApiError::Url(e.to_string()))?;
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in query {
                pairs.append_pair(k, v);
            }
        }

        let m = parse_method(method)?;
        let mut req = self.inner.request(m, url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            // The api emits JSON for everything except the binary stream
            // endpoints; those have dedicated MCP tools that don't go through
            // here. If decoding fails, surface the raw text so the caller
            // can debug.
            serde_json::from_slice::<Value>(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
        };
        Ok(ApiResponse { status, body })
    }

    pub async fn fetch_openapi(&self) -> Result<Value, ApiError> {
        let url = format!("{}/openapi.json", self.base_url);
        let resp = self
            .inner
            .get(&url)
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ApiError::Status {
                status: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        resp.json::<Value>()
            .await
            .map_err(|e| ApiError::Decode(e.to_string()))
    }
}

fn parse_method(s: &str) -> Result<Method, ApiError> {
    match s.to_ascii_uppercase().as_str() {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        "PUT" => Ok(Method::PUT),
        "PATCH" => Ok(Method::PATCH),
        "DELETE" => Ok(Method::DELETE),
        "HEAD" => Ok(Method::HEAD),
        "OPTIONS" => Ok(Method::OPTIONS),
        other => Err(ApiError::Url(format!("unsupported method `{other}`"))),
    }
}
