//! Songbook preview endpoints.
//!
//! `POST /songbook/preview-snippets` runs the same Tinyfish + yt-dlp
//! pipeline the create flow uses, but writes the output WAVs to a
//! per-request directory under `<storage>/_preview/<uuid>/`. The
//! response carries the directory id + per-clip metadata so the
//! frontend can render `<audio>` tags pointing at
//! `GET /songbook/preview/{preview_id}/{idx}/audio`.
//!
//! Authenticated; otherwise anyone could spend our yt-dlp budget.
//! Generated previews live on disk until a future cleanup pass —
//! per-clip storage is bounded (12 s × 6 ≈ 3.4 MB) so this is fine
//! for now. Add an `_preview/` sweep to the GC job if it ever
//! starts to bite.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use listenai_core::Error;
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::generation::song_snippets;
use crate::handlers::stream::StreamAuthQuery;
use crate::state::AppState;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PreviewSnippetsRequest {
    /// Same shape as the create-flow `topic` for songbooks — a
    /// song reference, e.g. `"Bohemian Rhapsody — Queen"`.
    #[validate(length(min = 3, max = 500))]
    pub topic: String,
    /// Number of evenly-spaced clips to download. Capped at 12.
    pub count: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewSnippetItem {
    /// 1-based clip index — matches the path component in the
    /// stream URL (`…/preview/{preview_id}/{index}/audio`).
    pub index: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewSnippetsResponse {
    /// UUID identifying the temporary preview dir under
    /// `<storage>/_preview/`. Pair with each item's `index` to build
    /// the stream URL: `/songbook/preview/{preview_id}/{index}/audio`.
    pub preview_id: String,
    /// YouTube URL the snippets were cut from. `None` if Tinyfish
    /// couldn't resolve the topic to a video.
    pub youtube_url: Option<String>,
    pub items: Vec<PreviewSnippetItem>,
    /// Human-readable failure reason when no clips were produced
    /// (yt-dlp missing, song too short, age-gated, …). `None` when
    /// at least one clip landed.
    pub error: Option<String>,
}

#[utoipa::path(
    post,
    path = "/songbook/preview-snippets",
    tag = "songbook",
    request_body = PreviewSnippetsRequest,
    responses(
        (status = 200, description = "Preview ready", body = PreviewSnippetsResponse),
        (status = 400, description = "Validation failed"),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn preview_snippets(
    State(state): State<AppState>,
    Authenticated(_user): Authenticated,
    Json(body): Json<PreviewSnippetsRequest>,
) -> ApiResult<Json<PreviewSnippetsResponse>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    let count = body.count.min(12);
    if count == 0 {
        return Err(Error::Validation("count must be ≥ 1".into()).into());
    }

    let preview_id = Uuid::new_v4().simple().to_string();
    let dir = state
        .config()
        .storage_path
        .join("_preview")
        .join(&preview_id);

    let outcome = song_snippets::download_into(&state, &dir, body.topic.trim(), count).await?;

    let items: Vec<PreviewSnippetItem> = outcome
        .produced
        .iter()
        .filter_map(|idx| {
            let path = dir.join(format!("snippet-{idx}.wav"));
            song_snippets::wav_duration_ms(&path)
                .ok()
                .map(|duration_ms| PreviewSnippetItem {
                    index: *idx,
                    duration_ms,
                })
        })
        .collect();

    Ok(Json(PreviewSnippetsResponse {
        preview_id,
        youtube_url: outcome.youtube_url,
        items,
        error: outcome.error,
    }))
}

#[utoipa::path(
    get,
    path = "/songbook/preview/{preview_id}/{idx}/audio",
    tag = "songbook",
    params(("preview_id" = String, Path), ("idx" = u32, Path)),
    responses(
        (status = 200, description = "WAV bytes", content_type = "audio/wav"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn preview_audio(
    State(state): State<AppState>,
    auth_header: Option<Authenticated>,
    Query(q): Query<StreamAuthQuery>,
    Path((preview_id, idx)): Path<(String, u32)>,
) -> ApiResult<Response> {
    // Auth is consistent with the chapter audio stream: header OR
    // ?access_token=… so a `<audio>` tag can play it directly.
    let _ = crate::handlers::stream::resolve_user(&state, auth_header, &q)?;

    // Path traversal guard: preview_id must be a hex-only UUID and
    // idx is constrained by the route param type.
    if !preview_id.chars().all(|c| c.is_ascii_hexdigit()) || preview_id.len() != 32 {
        return Err(Error::NotFound {
            resource: "preview".into(),
        }
        .into());
    }
    let path = state
        .config()
        .storage_path
        .join("_preview")
        .join(&preview_id)
        .join(format!("snippet-{idx}.wav"));
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| Error::NotFound {
            resource: format!("preview {preview_id}/{idx}"),
        })?;
    let len = file
        .metadata()
        .await
        .map(|m| m.len())
        .map_err(|e| Error::Other(anyhow::anyhow!("stat preview wav: {e}")))?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    Ok((StatusCode::OK, headers, body).into_response())
}
