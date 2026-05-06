//! Podcast CRUD + AI cover-image generation.
//!
//! A podcast is an owner-scoped grouping of audiobooks (see migration
//! 0029_podcast). Each podcast has a title, description, and an
//! AI-generated square cover image stored under
//! `<storage_path>/podcasts/<podcast_id>/cover.<ext>`.
//!
//! Phase 1 is local-only — the YouTube-playlist mirroring lives in
//! `youtube_playlist_id` (still NONE for now) and will be wired up by a
//! follow-up that creates the playlist on the user's connected channel
//! when `POST /podcasts` runs.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chrono::{DateTime, Utc};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::{tokens::verify_access_token, Authenticated};
use crate::error::ApiResult;
use crate::generation::cover as cover_gen;
use crate::handlers::cover::detect_mime;
use crate::handlers::stream::StreamAuthQuery;
use crate::state::AppState;
use crate::youtube::{account as yt_account, playlist as yt_playlist};

/// Privacy applied to playlists we mint from podcast metadata. We
/// deliberately ship them `public` so they're discoverable as soon as
/// the first video gets uploaded — single videos ship with their own
/// per-publish privacy_status, but a hidden playlist would obscure
/// even public videos. The user can flip privacy on YouTube directly
/// if they don't want that.
const PODCAST_PLAYLIST_PRIVACY: &str = "public";

// --- DTOs ----------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct PodcastRow {
    pub id: String,
    pub title: String,
    pub description: String,
    /// `true` when a cover image has been generated and is available at
    /// `GET /podcasts/:id/image`.
    pub has_image: bool,
    /// Number of audiobooks currently assigned to this podcast.
    pub audiobook_count: u32,
    /// YouTube playlist id mirrored from this podcast. `None` when YouTube
    /// isn't connected, or when the user hasn't synced yet.
    pub youtube_playlist_id: Option<String>,
    /// Convenience link, derived from `youtube_playlist_id`.
    pub youtube_playlist_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SyncPodcastResponse {
    /// `"created"` when a new playlist was minted, `"updated"` when an
    /// existing one was refreshed, `"unchanged"` when title/description
    /// hadn't drifted.
    pub action: String,
    pub youtube_playlist_id: Option<String>,
    pub youtube_playlist_url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PodcastList {
    pub items: Vec<PodcastRow>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreatePodcastRequest {
    #[validate(length(min = 1, max = 200))]
    pub title: String,
    #[validate(length(max = 4000))]
    pub description: Option<String>,
    /// Pre-generated cover image (raw base64, no `data:` prefix), produced
    /// by `POST /podcasts/preview-image`. Persisted to disk on create.
    pub image_base64: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdatePodcastRequest {
    #[validate(length(min = 1, max = 200))]
    pub title: Option<String>,
    #[validate(length(max = 4000))]
    pub description: Option<String>,
    /// Pass a fresh base64 to replace the cover, or `None` to leave the
    /// existing image in place. There's no "clear cover" path — every
    /// podcast keeps an image once one has been generated.
    pub image_base64: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PreviewPodcastImageRequest {
    #[validate(length(min = 1, max = 200))]
    pub title: String,
    #[validate(length(max = 4000))]
    pub description: Option<String>,
    /// Optional explicit image LLM id to use instead of the picker default.
    /// Same shape as the audiobook cover preview.
    #[validate(length(max = 64))]
    pub llm_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewPodcastImageResponse {
    /// Raw base64 (no `data:` prefix). MIME is reported separately so the
    /// UI can build the data URL itself.
    pub image_base64: String,
    pub mime_type: String,
}

// --- DB row --------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DbPodcast {
    id: Thing,
    owner: Thing,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    image_path: Option<String>,
    #[serde(default)]
    youtube_playlist_id: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl DbPodcast {
    fn to_row(&self, audiobook_count: u32) -> PodcastRow {
        let yt_id = self
            .youtube_playlist_id
            .clone()
            .filter(|s| !s.trim().is_empty());
        let yt_url = yt_id
            .as_deref()
            .map(|id| format!("https://www.youtube.com/playlist?list={id}"));
        PodcastRow {
            id: self.id.id.to_raw(),
            title: self.title.clone(),
            description: self.description.clone().unwrap_or_default(),
            has_image: self
                .image_path
                .as_deref()
                .map(|p| !p.trim().is_empty())
                .unwrap_or(false),
            audiobook_count,
            youtube_playlist_id: yt_id,
            youtube_playlist_url: yt_url,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    fn owner_id(&self) -> UserId {
        UserId(self.owner.id.to_raw())
    }
}

// --- Endpoints -----------------------------------------------------------

#[utoipa::path(
    get,
    path = "/podcasts",
    tag = "podcasts",
    responses(
        (status = 200, description = "Every podcast owned by the authed user", body = PodcastList),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<PodcastList>> {
    let rows: Vec<DbPodcast> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM podcast WHERE owner = user:`{}` ORDER BY created_at DESC",
            user.id.0,
        ))
        .await
        .map_err(|e| Error::Database(format!("list podcasts: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list podcasts (decode): {e}")))?;

    let counts = audiobook_counts_by_podcast(&state, &user.id).await?;
    let items = rows
        .iter()
        .map(|p| {
            let n = counts.get(&p.id.id.to_raw()).copied().unwrap_or(0);
            p.to_row(n)
        })
        .collect();
    Ok(Json(PodcastList { items }))
}

#[utoipa::path(
    post,
    path = "/podcasts",
    tag = "podcasts",
    request_body = CreatePodcastRequest,
    responses(
        (status = 200, description = "Newly created podcast", body = PodcastRow),
        (status = 400, description = "Validation failed"),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn create(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Json(body): Json<CreatePodcastRequest>,
) -> ApiResult<Json<PodcastRow>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    let id = Uuid::new_v4().simple().to_string();
    let title = body.title.trim().to_string();
    let description = body
        .description
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();

    state
        .db()
        .inner()
        .query(format!(
            r#"CREATE podcast:`{id}` CONTENT {{
                owner: user:`{uid}`,
                title: $t,
                description: $d
            }}"#,
            uid = user.id.0,
        ))
        .bind(("t", title))
        .bind(("d", description))
        .await
        .map_err(|e| Error::Database(format!("create podcast: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create podcast: {e}")))?;

    if let Some(b64) = body.image_base64.as_deref() {
        if let Err(e) = persist_image(&state, &id, b64).await {
            tracing::warn!(error = %e, podcast_id = id, "create: image persist failed");
        }
    }

    // Best-effort YouTube playlist mint. Failure here logs but doesn't
    // block the create — the user can `POST /podcasts/:id/sync-youtube`
    // later to retry, and the local podcast row is otherwise functional.
    sync_youtube_playlist(&state, &id, &user.id).await;

    let row = load_owned(&state, &id, &user.id).await?;
    let count = audiobook_count_for_podcast(&state, &id).await?;
    Ok(Json(row.to_row(count)))
}

#[utoipa::path(
    get,
    path = "/podcasts/{id}",
    tag = "podcasts",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "Podcast", body = PodcastRow),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn get_one(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<Json<PodcastRow>> {
    let row = load_owned(&state, &id, &user.id).await?;
    let count = audiobook_count_for_podcast(&state, &id).await?;
    Ok(Json(row.to_row(count)))
}

#[utoipa::path(
    patch,
    path = "/podcasts/{id}",
    tag = "podcasts",
    params(("id" = String, Path)),
    request_body = UpdatePodcastRequest,
    responses(
        (status = 200, description = "Updated podcast", body = PodcastRow),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn patch(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    Json(body): Json<UpdatePodcastRequest>,
) -> ApiResult<Json<PodcastRow>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    // Confirm ownership before any mutation lands.
    load_owned(&state, &id, &user.id).await?;

    if let Some(title) = body.title {
        state
            .db()
            .inner()
            .query(format!("UPDATE podcast:`{id}` SET title = $t"))
            .bind(("t", title.trim().to_string()))
            .await
            .map_err(|e| Error::Database(format!("patch podcast title: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch podcast title: {e}")))?;
    }
    if let Some(desc) = body.description {
        state
            .db()
            .inner()
            .query(format!("UPDATE podcast:`{id}` SET description = $d"))
            .bind(("d", desc.trim().to_string()))
            .await
            .map_err(|e| Error::Database(format!("patch podcast description: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch podcast description: {e}")))?;
    }
    if let Some(b64) = body
        .image_base64
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        persist_image(&state, &id, b64).await?;
    }

    // Mirror title/description changes to YouTube. Best-effort: a YT
    // failure shouldn't roll back the local update.
    sync_youtube_playlist(&state, &id, &user.id).await;

    let row = load_owned(&state, &id, &user.id).await?;
    let count = audiobook_count_for_podcast(&state, &id).await?;
    Ok(Json(row.to_row(count)))
}

#[utoipa::path(
    delete,
    path = "/podcasts/{id}",
    tag = "podcasts",
    params(("id" = String, Path)),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn delete(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let row = load_owned(&state, &id, &user.id).await?;

    // Best-effort delete the linked YouTube playlist before we drop the
    // local row, so the YT side doesn't end up with an orphan we no
    // longer remember about.
    if let Some(playlist_id) = row
        .youtube_playlist_id
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        match yt_account::access_token(&state, &user.id).await {
            Ok(Some(token)) => {
                if let Err(e) = yt_playlist::delete_playlist(&token, playlist_id).await {
                    tracing::warn!(error = %e, podcast_id = id, "delete: yt playlist delete failed");
                }
            }
            Ok(None) => {} // YT not connected — nothing to clean up.
            Err(Error::Unauthorized) => {
                yt_account::drop_account(&state, &user.id).await.ok();
            }
            Err(e) => {
                tracing::warn!(error = %e, podcast_id = id, "delete: yt access token failed");
            }
        }
    }

    // Move any audiobooks pointing at this podcast back to "no podcast"
    // before the row goes away — the audiobook field is `option<record>`
    // so a dangling reference would otherwise resolve to NONE on read but
    // still serialise unhelpfully on writes.
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook SET podcast = NONE \
             WHERE owner = user:`{uid}` AND podcast = podcast:`{id}`",
            uid = user.id.0,
        ))
        .await
        .map_err(|e| Error::Database(format!("clear podcast refs: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("clear podcast refs: {e}")))?;

    state
        .db()
        .inner()
        .query(format!("DELETE podcast:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("delete podcast: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("delete podcast: {e}")))?;

    // Best-effort cleanup of the image dir on disk.
    let dir = state.config().storage_path.join("podcasts").join(&id);
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(error = %e, ?dir, "delete podcast: remove image dir failed");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/podcasts/preview-image",
    tag = "podcasts",
    request_body = PreviewPodcastImageRequest,
    responses(
        (status = 200, description = "Generated cover", body = PreviewPodcastImageResponse),
        (status = 400, description = "Validation failed"),
        (status = 401, description = "Unauthenticated"),
        (status = 502, description = "Upstream image-gen error")
    ),
    security(("bearer" = []))
)]
pub async fn preview_image(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Json(body): Json<PreviewPodcastImageRequest>,
) -> ApiResult<Json<PreviewPodcastImageResponse>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    let bytes = cover_gen::generate_podcast(
        &state,
        &user.id,
        &body.title,
        body.description.as_deref(),
        body.llm_id.as_deref(),
    )
    .await?;
    Ok(Json(PreviewPodcastImageResponse {
        image_base64: B64.encode(&bytes),
        mime_type: detect_mime(&bytes).to_string(),
    }))
}

#[utoipa::path(
    get,
    path = "/podcasts/{id}/image",
    tag = "podcasts",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "Cover bytes", content_type = "image/png"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn image(
    State(state): State<AppState>,
    auth_header: Option<Authenticated>,
    Query(q): Query<StreamAuthQuery>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let user_id = resolve_user(&state, auth_header, &q)?;
    let row = load_owned(&state, &id, &user_id).await?;

    let rel = row
        .image_path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .ok_or(Error::NotFound {
            resource: format!("image for podcast:{id}"),
        })?;
    let abs = state.config().storage_path.join(&rel);
    let file = tokio::fs::File::open(&abs)
        .await
        .map_err(|_| Error::NotFound {
            resource: format!("image for podcast:{id}"),
        })?;
    let len = file
        .metadata()
        .await
        .map(|m| m.len())
        .map_err(|e| Error::Other(anyhow::anyhow!("stat podcast image: {e}")))?;

    let mut head = [0u8; 32];
    let head_len = match tokio::fs::File::open(&abs).await {
        Ok(mut f) => f.read(&mut head).await.unwrap_or(0),
        Err(_) => 0,
    };
    let mime = detect_mime(&head[..head_len]);

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

#[utoipa::path(
    post,
    path = "/podcasts/{id}/sync-youtube",
    tag = "podcasts",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "Playlist created or refreshed", body = SyncPodcastResponse),
        (status = 404, description = "Not found"),
        (status = 409, description = "YouTube is not connected for this user"),
        (status = 502, description = "Upstream YouTube error")
    ),
    security(("bearer" = []))
)]
pub async fn sync_youtube(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<Json<SyncPodcastResponse>> {
    // Same flow as the implicit best-effort sync, but errors propagate so
    // the user sees them on a manual click.
    load_owned(&state, &id, &user.id).await?;

    let token = match yt_account::access_token(&state, &user.id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Err(Error::Conflict("connect a YouTube channel first".into()).into()),
        Err(Error::Unauthorized) => {
            yt_account::drop_account(&state, &user.id).await.ok();
            return Err(
                Error::Conflict("YouTube reconnect required (token rejected)".into()).into(),
            );
        }
        Err(e) => return Err(e.into()),
    };

    let action = sync_with_token(&state, &id, &token).await?;
    let row = load_owned(&state, &id, &user.id).await?;
    Ok(Json(SyncPodcastResponse {
        action: action.to_string(),
        youtube_playlist_id: row
            .youtube_playlist_id
            .clone()
            .filter(|s| !s.trim().is_empty()),
        youtube_playlist_url: row
            .youtube_playlist_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|p| format!("https://www.youtube.com/playlist?list={p}")),
    }))
}

// --- Internal helpers ----------------------------------------------------

async fn load_owned(state: &AppState, id: &str, user: &UserId) -> Result<DbPodcast> {
    let rows: Vec<DbPodcast> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM podcast:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("load podcast: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load podcast (decode): {e}")))?;
    let row = rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("podcast:{id}"),
    })?;
    if row.owner_id() != *user {
        // Don't leak existence to non-owners.
        return Err(Error::NotFound {
            resource: format!("podcast:{id}"),
        });
    }
    Ok(row)
}

async fn audiobook_count_for_podcast(state: &AppState, id: &str) -> Result<u32> {
    #[derive(Deserialize)]
    struct Row {
        count: i64,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM audiobook \
             WHERE podcast = podcast:`{id}` GROUP ALL"
        ))
        .await
        .map_err(|e| Error::Database(format!("podcast count: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("podcast count (decode): {e}")))?;
    Ok(rows
        .into_iter()
        .next()
        .map(|r| r.count.max(0) as u32)
        .unwrap_or(0))
}

async fn audiobook_counts_by_podcast(
    state: &AppState,
    user: &UserId,
) -> Result<std::collections::HashMap<String, u32>> {
    #[derive(Deserialize)]
    struct Row {
        podcast: Option<Thing>,
        #[serde(default)]
        count: Option<i64>,
    }
    // Pull every (audiobook → podcast) pointer for this user; group in
    // Rust because SurrealDB's `GROUP BY` on a record link gets fiddly.
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT podcast FROM audiobook \
             WHERE owner = user:`{uid}` AND podcast != NONE",
            uid = user.0,
        ))
        .await
        .map_err(|e| Error::Database(format!("podcast counts: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("podcast counts (decode): {e}")))?;
    let mut counts = std::collections::HashMap::new();
    for r in rows {
        if let Some(p) = r.podcast {
            *counts.entry(p.id.to_raw()).or_insert(0u32) += r.count.unwrap_or(1).max(0) as u32;
        }
    }
    Ok(counts)
}

/// Decode a base64 cover, write it under
/// `<storage_path>/podcasts/<podcast_id>/cover.<ext>`, and update
/// `image_path` on the row. Tolerates `data:image/...;base64,` prefixes.
async fn persist_image(state: &AppState, podcast_id: &str, b64: &str) -> Result<()> {
    let trimmed = b64.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let raw = trimmed
        .find(";base64,")
        .map(|i| &trimmed[(i + ";base64,".len())..])
        .unwrap_or(trimmed);
    let bytes = B64
        .decode(raw.as_bytes())
        .map_err(|e| Error::Validation(format!("image_base64: {e}")))?;
    if bytes.is_empty() {
        return Err(Error::Validation("image_base64 is empty".into()));
    }
    let ext = match detect_mime(&bytes) {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "bin",
    };
    let dir = state
        .config()
        .storage_path
        .join("podcasts")
        .join(podcast_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Other(anyhow::anyhow!("create podcast image dir {dir:?}: {e}")))?;
    let filename = format!("cover.{ext}");
    let path = dir.join(&filename);
    std::fs::write(&path, &bytes)
        .map_err(|e| Error::Other(anyhow::anyhow!("write podcast image {path:?}: {e}")))?;

    let rel = format!("podcasts/{podcast_id}/{filename}");
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE podcast:`{podcast_id}` SET image_path = $p, updated_at = time::now()"
        ))
        .bind(("p", rel))
        .await
        .map_err(|e| Error::Database(format!("set podcast image_path: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set podcast image_path: {e}")))?;
    Ok(())
}

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

/// Best-effort sync wrapper used by create/patch. Resolves the user's
/// YouTube account; if absent, does nothing. If the upstream call fails,
/// logs a warning. Errors never reach the caller — the local row is the
/// source of truth and the user can retry via `POST /sync-youtube`.
async fn sync_youtube_playlist(state: &AppState, podcast_id: &str, user: &UserId) {
    let token = match yt_account::access_token(state, user).await {
        Ok(Some(t)) => t,
        Ok(None) => return,
        Err(Error::Unauthorized) => {
            yt_account::drop_account(state, user).await.ok();
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, podcast_id, "podcast sync: token load failed");
            return;
        }
    };
    if let Err(e) = sync_with_token(state, podcast_id, &token).await {
        tracing::warn!(error = %e, podcast_id, "podcast sync: yt sync failed");
    }
}

/// Action taken by `sync_with_token`. Surfaced to the manual sync
/// endpoint so the UI can show "created" vs "refreshed".
enum SyncAction {
    Created,
    Updated,
    Unchanged,
}

impl std::fmt::Display for SyncAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SyncAction::Created => "created",
            SyncAction::Updated => "updated",
            SyncAction::Unchanged => "unchanged",
        })
    }
}

/// Mint a new playlist if the podcast doesn't have one, otherwise update
/// the existing playlist's metadata. Persists the playlist id on the
/// row when freshly minted. Caller is responsible for resolving the
/// access token.
async fn sync_with_token(state: &AppState, podcast_id: &str, token: &str) -> Result<SyncAction> {
    #[derive(Deserialize)]
    struct Row {
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        youtube_playlist_id: Option<String>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT title, description, youtube_playlist_id FROM podcast:`{podcast_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("podcast sync load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("podcast sync load (decode): {e}")))?;
    let row = rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("podcast:{podcast_id}"),
    })?;

    let title = row.title.trim().chars().take(150).collect::<String>();
    let description = row
        .description
        .unwrap_or_default()
        .chars()
        .take(5000)
        .collect::<String>();

    if let Some(existing) = row
        .youtube_playlist_id
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        // Try with podcast designation first. YouTube rejects this with
        // `failedPrecondition` (mapped to Conflict) when the playlist is
        // empty — full-length episodes are required before designation.
        // Fall through to a regular update so the metadata still lands,
        // then surface a Conflict so the caller knows to publish first.
        match yt_playlist::update_playlist(
            token,
            existing,
            &title,
            &description,
            PODCAST_PLAYLIST_PRIVACY,
            None,
            true,
        )
        .await
        {
            Ok(()) => Ok(SyncAction::Updated),
            // Playlist deleted on YouTube → drop our reference and mint
            // a fresh one in the same call so the user gets a working
            // link without a second click.
            Err(Error::NotFound { .. }) => {
                clear_playlist_id(state, podcast_id).await.ok();
                let p = yt_playlist::create_playlist(
                    token,
                    &title,
                    &description,
                    PODCAST_PLAYLIST_PRIVACY,
                    None,
                    false,
                )
                .await?;
                save_playlist_id(state, podcast_id, &p.id).await?;
                Ok(SyncAction::Created)
            }
            Err(Error::Conflict(_)) => {
                // Apply title/description anyway so the user's edits are
                // visible on YouTube, then bubble a friendlier error.
                yt_playlist::update_playlist(
                    token,
                    existing,
                    &title,
                    &description,
                    PODCAST_PLAYLIST_PRIVACY,
                    None,
                    false,
                )
                .await
                .ok();
                Err(Error::Conflict(
                    "YouTube requires at least one published episode before \
                     designating a playlist as a podcast. Publish an audiobook \
                     into this podcast first, then re-sync."
                        .into(),
                ))
            }
            Err(e) => Err(e),
        }
    } else {
        // Fresh mint: create as a regular playlist. We'll flip to a
        // podcast (via the same `update_playlist` path) once the first
        // video lands in it — see `try_designate_podcast` in the publish
        // job. Designating up front would 400 with failedPrecondition
        // because the playlist is still empty.
        let playlist = yt_playlist::create_playlist(
            token,
            &title,
            &description,
            PODCAST_PLAYLIST_PRIVACY,
            None,
            false,
        )
        .await?;
        save_playlist_id(state, podcast_id, &playlist.id).await?;
        // The Unchanged variant is reserved for "no upstream call needed".
        let _ = SyncAction::Unchanged;
        Ok(SyncAction::Created)
    }
}

async fn save_playlist_id(state: &AppState, podcast_id: &str, playlist_id: &str) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE podcast:`{podcast_id}` SET \
                youtube_playlist_id = $pid, \
                updated_at = time::now()"
        ))
        .bind(("pid", playlist_id.to_string()))
        .await
        .map_err(|e| Error::Database(format!("save podcast playlist_id: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("save podcast playlist_id: {e}")))?;
    Ok(())
}

async fn clear_playlist_id(state: &AppState, podcast_id: &str) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE podcast:`{podcast_id}` SET \
                youtube_playlist_id = NONE, \
                updated_at = time::now()"
        ))
        .await
        .map_err(|e| Error::Database(format!("clear podcast playlist_id: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("clear podcast playlist_id: {e}")))?;
    Ok(())
}
