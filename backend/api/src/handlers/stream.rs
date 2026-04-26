//! Binary streaming endpoints for audio + waveform.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use surrealdb::sql::Thing;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use crate::auth::{tokens::verify_access_token, Authenticated};
use crate::error::ApiResult;
use crate::state::AppState;

/// Query param accepted on binary stream endpoints. The browser's `<audio>`
/// tag can't attach an `Authorization` header on its own, so we accept the
/// access token here too (same pattern as `/ws/audiobook/:id`).
#[derive(Debug, Deserialize, Default)]
pub struct StreamAuthQuery {
    pub access_token: Option<String>,
    /// Optional language filter (matches `chapter.language`). Defaults to
    /// the audiobook's primary language.
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OwnerRow {
    owner: Thing,
}

#[derive(Debug, Deserialize)]
struct AudioRow {
    audio_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoverRow {
    cover_path: Option<String>,
}

#[utoipa::path(
    get,
    path = "/audiobook/{id}/chapter/{n}/audio",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 200, description = "WAV bytes", content_type = "audio/wav"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn chapter_audio(
    State(state): State<AppState>,
    auth_header: Option<Authenticated>,
    Query(q): Query<StreamAuthQuery>,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Response> {
    let user_id = resolve_user(&state, auth_header, &q)?;
    assert_owner(&state, &id, &user_id).await?;
    let audio_path = load_audio_path(&state, &id, n as i64, q.language.as_deref()).await?;
    let file = tokio::fs::File::open(&audio_path)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("open audio: {e}")))?;
    let len = file
        .metadata()
        .await
        .map(|m| m.len())
        .map_err(|e| Error::Other(anyhow::anyhow!("stat audio: {e}")))?;
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

#[utoipa::path(
    get,
    path = "/audiobook/{id}/chapter/{n}/waveform",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 200, description = "JSON peaks"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn chapter_waveform(
    State(state): State<AppState>,
    auth_header: Option<Authenticated>,
    Query(q): Query<StreamAuthQuery>,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Json<serde_json::Value>> {
    let user_id = resolve_user(&state, auth_header, &q)?;
    assert_owner(&state, &id, &user_id).await?;
    let audio_path = load_audio_path(&state, &id, n as i64, q.language.as_deref()).await?;
    let waveform_path = audio_path.replace(".wav", ".waveform.json");

    let mut file = tokio::fs::File::open(&waveform_path)
        .await
        .map_err(|_| Error::NotFound {
            resource: format!("waveform for audiobook:{id} chapter {n}"),
        })?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("read waveform: {e}")))?;
    let v: serde_json::Value = serde_json::from_str(&buf)
        .map_err(|e| Error::Other(anyhow::anyhow!("parse waveform: {e}")))?;
    Ok(Json(v))
}

#[utoipa::path(
    get,
    path = "/audiobook/{id}/cover",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "Cover image bytes", content_type = "image/png"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn cover(
    State(state): State<AppState>,
    auth_header: Option<Authenticated>,
    Query(q): Query<StreamAuthQuery>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let user_id = resolve_user(&state, auth_header, &q)?;
    assert_owner(&state, &id, &user_id).await?;

    let rel = load_cover_rel(&state, &id).await?;
    let abs = state.config().storage_path.join(&rel);
    let file = tokio::fs::File::open(&abs)
        .await
        .map_err(|_| Error::NotFound {
            resource: format!("cover for audiobook:{id}"),
        })?;
    let len = file
        .metadata()
        .await
        .map(|m| m.len())
        .map_err(|e| Error::Other(anyhow::anyhow!("stat cover: {e}")))?;

    // Sniff first 32 bytes for the right Content-Type.
    let mut head = [0u8; 32];
    let head_len = match tokio::fs::File::open(&abs).await {
        Ok(mut f) => f.read(&mut head).await.unwrap_or(0),
        Err(_) => 0,
    };
    let mime = crate::handlers::cover::detect_mime(&head[..head_len]);

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime).unwrap_or_else(|_| HeaderValue::from_static("image/png")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    Ok((StatusCode::OK, headers, body).into_response())
}

async fn load_cover_rel(state: &AppState, audiobook_id: &str) -> Result<String> {
    let rows: Vec<CoverRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT cover_path FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("load cover_path: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load cover_path (decode): {e}")))?;
    rows.into_iter()
        .next()
        .and_then(|r| r.cover_path)
        .filter(|p| !p.trim().is_empty())
        .ok_or(Error::NotFound {
            resource: format!("cover for audiobook:{audiobook_id}"),
        })
}

/// Accept either the `Authorization: Bearer …` header (standard) OR the
/// `?access_token=` query param (for `<audio>` / `<img>` consumers that can't
/// set headers).
fn resolve_user(
    state: &AppState,
    header_auth: Option<Authenticated>,
    q: &StreamAuthQuery,
) -> Result<UserId> {
    if let Some(Authenticated(user)) = header_auth {
        return Ok(user.id);
    }
    let token = q.access_token.as_deref().ok_or(Error::Unauthorized)?;
    let claims = verify_access_token(token, &state.config().jwt_secret)?;
    Ok(claims.sub)
}

async fn assert_owner(state: &AppState, audiobook_id: &str, user: &UserId) -> Result<()> {
    let rows: Vec<OwnerRow> = state
        .db()
        .inner()
        .query(format!("SELECT owner FROM audiobook:`{audiobook_id}`"))
        .await
        .map_err(|e| Error::Database(format!("assert owner: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("assert owner (decode): {e}")))?;
    let row = rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{audiobook_id}"),
    })?;
    if row.owner.id.to_raw() != user.0 {
        return Err(Error::NotFound {
            resource: format!("audiobook:{audiobook_id}"),
        });
    }
    Ok(())
}

async fn load_audio_path(
    state: &AppState,
    audiobook_id: &str,
    n: i64,
    language: Option<&str>,
) -> Result<String> {
    // Default to the audiobook's primary language so older clients (no
    // ?language query) keep working unchanged.
    let lang = match language {
        Some(l) if !l.trim().is_empty() => l.to_string(),
        _ => primary_language(state, audiobook_id).await?,
    };
    let rows: Vec<AudioRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT audio_path FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", n))
        .bind(("lang", lang))
        .await
        .map_err(|e| Error::Database(format!("load audio path: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load audio path (decode): {e}")))?;
    rows.into_iter()
        .next()
        .and_then(|r| r.audio_path)
        .ok_or(Error::NotFound {
            resource: format!("audio for audiobook:{audiobook_id} chapter {n}"),
        })
}

async fn primary_language(state: &AppState, audiobook_id: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        language: Option<String>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT language FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("primary lang: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("primary lang (decode): {e}")))?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.language)
        .unwrap_or_else(|| "en".to_string()))
}
