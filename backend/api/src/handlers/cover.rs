//! Cover-art preview endpoint.
//!
//! Stateless: takes a topic + optional genre, returns a base64 PNG. The UI
//! displays this inline, then carries the bytes through to `POST /audiobook`
//! where they get persisted alongside the new audiobook record.

use axum::{extract::State, Json};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use listenai_core::Error;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::generation::cover as cover_gen;
use crate::state::AppState;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CoverPreviewRequest {
    #[validate(length(min = 3, max = 500))]
    pub topic: String,
    #[validate(length(max = 40))]
    pub genre: Option<String>,
    /// Visual style hint, e.g. `"watercolor"`, `"cartoon"`. The frontend
    /// offers a curated dropdown but free-text is accepted. When omitted,
    /// the generator falls back to its built-in default.
    #[validate(length(max = 60))]
    pub art_style: Option<String>,
    /// Optional explicit LLM id to use instead of the picker's default.
    /// Surfaced on the New Audiobook page as a dropdown when more than one
    /// image-capable model is configured. Server validates the id against
    /// the `llm` table.
    #[validate(length(max = 64))]
    pub llm_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CoverPreviewResponse {
    /// Raw base64 (no `data:` prefix). MIME is reported separately so the UI
    /// can build the data URL itself.
    pub image_base64: String,
    pub mime_type: String,
}

#[utoipa::path(
    post,
    path = "/cover-art/preview",
    tag = "cover-art",
    request_body = CoverPreviewRequest,
    responses(
        (status = 200, description = "Generated cover", body = CoverPreviewResponse),
        (status = 400, description = "Validation failed"),
        (status = 401, description = "Unauthenticated"),
        (status = 502, description = "Upstream image-gen error")
    ),
    security(("bearer" = []))
)]
pub async fn preview(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Json(body): Json<CoverPreviewRequest>,
) -> ApiResult<Json<CoverPreviewResponse>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    let bytes = cover_gen::generate(
        &state,
        &user.id,
        None,
        &body.topic,
        body.genre.as_deref(),
        body.art_style.as_deref(),
        body.llm_id.as_deref(),
    )
    .await?;
    Ok(Json(CoverPreviewResponse {
        image_base64: B64.encode(&bytes),
        mime_type: detect_mime(&bytes).to_string(),
    }))
}

/// Best-effort MIME sniff for the common image formats. The OpenRouter
/// image route almost always returns PNG, but JPEG / WebP would parse too.
pub fn detect_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}
