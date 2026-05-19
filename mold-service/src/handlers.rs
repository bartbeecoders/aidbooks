//! HTTP handlers exposed by mold-service. Public shape:
//!
//! - `GET /healthz` — service liveness + upstream reachability
//! - `POST /v1/generate` — generate an image (returns base64)
//! - `POST /v1/models/pull` — pull a model on the upstream
//! - `DELETE /v1/models/unload` — drop loaded models from VRAM
//! - `GET /v1/defaults` — preview the policy defaults for a given model
//!
//! Auth: when `MOLD_SERVICE_API_KEY` is set, every non-`/healthz`
//! request must carry `X-Api-Key: <value>`.

use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};

use crate::client;
use crate::error::{Error, Result};
use crate::policy;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct GenerateRequest {
    pub prompt: String,
    /// Mold model slug (e.g. `flux2-klein:q8`). Defaults to
    /// `policy::DEFAULT_MOLD_MODEL` when omitted or blank.
    #[serde(default)]
    pub model: Option<String>,
    /// When `true` (and `width`/`height` not given), use the 9:16
    /// short-form aspect; otherwise square. Ignored if explicit
    /// dimensions are passed.
    #[serde(default)]
    pub is_short: Option<bool>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Override the per-model step default. Most callers should leave
    /// this unset and let the service pick.
    #[serde(default)]
    pub steps: Option<u32>,
    #[serde(default)]
    pub guidance: Option<f64>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub negative_prompt: Option<String>,
    /// `png` (default), `jpeg`, `webp`. Maps to the mold response
    /// `Content-Type` and the returned `content_type` field.
    #[serde(default)]
    pub output_format: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GenerateResponse {
    pub image_base64: String,
    pub content_type: String,
    pub width: u32,
    pub height: u32,
    pub model: String,
    pub steps: u32,
    pub guidance: Option<f64>,
    pub seed_used: Option<i64>,
    pub output_format: String,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub upstream_reachable: bool,
    pub upstream_url: String,
    pub version: &'static str,
    pub max_concurrency: usize,
    pub available_permits: usize,
}

#[derive(Debug, Deserialize)]
pub struct DefaultsQuery {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub is_short: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct DefaultsResponse {
    pub model: String,
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub guidance: Option<f64>,
}

fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<()> {
    let Some(expected) = state.config.api_key.as_deref() else {
        return Ok(());
    };
    let got = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // Constant-time-ish: equal-length compare is fine here — the key
    // is server-set and the timing channel is dwarfed by the 5–60s
    // generate call. If you need stricter, swap in `subtle`.
    if got == expected {
        Ok(())
    } else {
        Err(Error::Unauthorized)
    }
}

pub async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    let reachable = client::health(&state.config.upstream_url).await;
    Json(HealthResponse {
        status: "ok",
        upstream_reachable: reachable,
        upstream_url: state.config.upstream_url.clone(),
        version: env!("CARGO_PKG_VERSION"),
        max_concurrency: state.config.max_concurrency,
        available_permits: state.semaphore.available_permits(),
    })
}

pub async fn defaults(
    State(_state): State<AppState>,
    Query(q): Query<DefaultsQuery>,
) -> Json<DefaultsResponse> {
    let model = match q.model.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(m) => m.to_string(),
        None => policy::DEFAULT_MOLD_MODEL.to_string(),
    };
    let (width, height) = policy::dimensions_for(q.is_short.unwrap_or(false));
    let steps = policy::default_steps_for(&model);
    let guidance = policy::default_guidance_for(&model);
    Json(DefaultsResponse {
        model,
        width,
        height,
        steps,
        guidance,
    })
}

pub async fn generate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<GenerateRequest>,
) -> Result<Json<GenerateResponse>> {
    check_auth(&state, &headers)?;

    if req.prompt.trim().is_empty() {
        return Err(Error::BadRequest("prompt is required".into()));
    }

    let model = match req.model.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(m) => m.to_string(),
        None => policy::DEFAULT_MOLD_MODEL.to_string(),
    };
    let (w_def, h_def) = policy::dimensions_for(req.is_short.unwrap_or(false));
    let width = req.width.unwrap_or(w_def);
    let height = req.height.unwrap_or(h_def);
    if width == 0 || height == 0 {
        return Err(Error::BadRequest(
            "width and height must be > 0".into(),
        ));
    }
    if width % 16 != 0 || height % 16 != 0 {
        return Err(Error::BadRequest(
            "width and height must be multiples of 16".into(),
        ));
    }
    let steps = req.steps.unwrap_or_else(|| policy::default_steps_for(&model));
    let guidance = req.guidance.or_else(|| policy::default_guidance_for(&model));
    let output_format = req
        .output_format
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("png")
        .to_string();

    let upstream_req = client::UpstreamGenerateRequest {
        prompt: req.prompt,
        model: model.clone(),
        width,
        height,
        steps,
        seed: req.seed,
        guidance,
        negative_prompt: req.negative_prompt,
        output_format: output_format.clone(),
    };

    let _permit = state
        .semaphore
        .acquire()
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("semaphore: {e}")))?;

    let result = client::generate(
        &state.config.upstream_url,
        state.config.upstream_api_key.as_deref(),
        state.config.timeout_secs,
        &upstream_req,
    )
    .await;

    match result {
        Ok(resp) => Ok(Json(GenerateResponse {
            image_base64: B64.encode(&resp.bytes),
            content_type: resp.content_type,
            width,
            height,
            model,
            steps,
            guidance,
            seed_used: resp.seed_used,
            output_format,
        })),
        Err(Error::Upstream(msg)) if policy::is_oom_error(&msg) => {
            // Mold marks a worker degraded after 3 consecutive failures
            // and refuses every request for 60s. When we OOM at the
            // application layer in <30ms the queue worker burns through
            // all 3 strikes before it can back off. Hold the semaphore
            // past mold's cooldown so the next request from this
            // service sees a fresh worker again.
            let cooldown = state.config.oom_cooldown_secs;
            tracing::warn!(
                upstream = %state.config.upstream_url,
                cooldown_secs = cooldown,
                "mold OOM detected; holding semaphore past mold's degrade cooldown"
            );
            tokio::time::sleep(Duration::from_secs(cooldown)).await;
            Err(Error::Upstream(msg))
        }
        Err(e) => Err(e),
    }
}

pub async fn pull(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PullRequest>,
) -> Result<Json<MessageResponse>> {
    check_auth(&state, &headers)?;
    if req.model.trim().is_empty() {
        return Err(Error::BadRequest("model is required".into()));
    }
    tracing::info!(model = %req.model, "pulling mold model");
    let message = client::pull(
        &state.config.upstream_url,
        state.config.upstream_api_key.as_deref(),
        state.config.pull_timeout_secs,
        &req.model,
    )
    .await?;
    Ok(Json(MessageResponse { message }))
}

pub async fn unload(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MessageResponse>> {
    check_auth(&state, &headers)?;
    tracing::info!("unloading mold models");
    let message = client::unload(
        &state.config.upstream_url,
        state.config.upstream_api_key.as_deref(),
    )
    .await?;
    Ok(Json(MessageResponse { message }))
}
