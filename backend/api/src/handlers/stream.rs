//! Binary streaming endpoints for audio + waveform.

use axum::{
    body::Body,
    extract::{Path, State},
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

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct OwnerRow {
    owner: Thing,
}

#[derive(Debug, Deserialize)]
struct AudioRow {
    audio_path: Option<String>,
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
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Response> {
    assert_owner(&state, &id, &user.id).await?;
    let audio_path = load_audio_path(&state, &id, n as i64).await?;
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
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Json<serde_json::Value>> {
    assert_owner(&state, &id, &user.id).await?;
    let audio_path = load_audio_path(&state, &id, n as i64).await?;
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

async fn load_audio_path(state: &AppState, audiobook_id: &str, n: i64) -> Result<String> {
    let rows: Vec<AudioRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT audio_path FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` AND number = $n LIMIT 1"
        ))
        .bind(("n", n))
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
