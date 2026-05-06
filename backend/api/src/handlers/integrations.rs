//! Third-party integrations — currently just YouTube publishing.
//!
//! The OAuth dance is split across two endpoints:
//!   * `GET /integrations/youtube/oauth/start` — authenticated; returns the
//!     consent URL the SPA navigates to. We don't 302 directly because the
//!     browser drops `Authorization` headers across a redirect.
//!   * `GET /integrations/youtube/oauth/callback` — UNAUTHENTICATED; Google
//!     can't carry our JWT through its redirect, so we identify the returning
//!     user via the `state` parameter we issued at /start.
//!
//! Plus per-account introspection and disconnect, and the publish trigger.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use listenai_core::domain::JobKind;
use listenai_core::id::{AudiobookId, UserId};
use listenai_core::{Error, Result};
use listenai_jobs::repo::EnqueueRequest;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use utoipa::ToSchema;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::state::AppState;
use crate::youtube::{encrypt, oauth};

// --- DTOs ---------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct OauthStartResponse {
    /// URL to navigate the user to (with `window.location = ...`). We don't
    /// 302 from this endpoint because the browser drops `Authorization`
    /// headers across same-origin redirects.
    pub url: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct YoutubeAccountStatus {
    pub connected: bool,
    /// Channel name to display in the UI; `None` when not connected.
    pub channel_title: Option<String>,
    /// `connected_at` ISO timestamp, for "connected since X" copy.
    pub connected_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PublishYoutubeRequest {
    /// Audiobook language version to publish. Must already be `audio_ready`.
    pub language: String,
    /// `private`, `unlisted`, or `public`. Defaults to `private` when absent.
    #[serde(default)]
    pub privacy_status: Option<String>,
    /// `single` (one concatenated video, the default) or `playlist` (one
    /// video per chapter, all added to a new YouTube playlist).
    #[serde(default)]
    pub mode: Option<String>,
    /// When true, encode the MP4(s) but pause before uploading so the user
    /// can preview locally and explicitly approve. Defaults to false.
    #[serde(default)]
    pub review: Option<bool>,
    /// When true, use the per-chapter animated companion videos
    /// (`<storage>/<book>/<lang>/ch-<n>.video.mp4`, produced by the
    /// `animate` job) as the visual track instead of the static cover
    /// loop. Defaults to false.
    #[serde(default)]
    pub animate: Option<bool>,
    /// Optional override for the YouTube video description; falls back to a
    /// generated one (topic + chapter list with timestamps).
    #[serde(default)]
    pub description: Option<String>,
    /// Per-video override for the "Like & Subscribe!" overlay.
    ///   `None`  → inherit the global setting (admin → YouTube settings).
    ///   `Some(true)` / `Some(false)` → force on/off for this publication.
    #[serde(default)]
    pub like_subscribe_overlay: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublishYoutubeResponse {
    pub job_id: String,
    pub publication_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublicationVideoRow {
    pub chapter_number: i64,
    pub title: String,
    pub video_id: Option<String>,
    pub video_url: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublicationRow {
    pub id: String,
    pub language: String,
    pub privacy_status: String,
    /// `"single"` or `"playlist"` (defaults to `"single"` for legacy rows).
    pub mode: String,
    /// True while the publication is awaiting an explicit `approve` call
    /// before uploading. Always paired with `preview_ready_at` once the
    /// encoder finishes.
    pub review: bool,
    /// Timestamp at which encoding completed and the MP4 is streamable
    /// from `/audiobook/:id/publications/:pid/preview`. Cleared on approve.
    pub preview_ready_at: Option<DateTime<Utc>>,
    pub video_id: Option<String>,
    pub video_url: Option<String>,
    /// Set when `mode = "playlist"` and the playlist has been created.
    pub playlist_id: Option<String>,
    pub playlist_url: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Per-chapter videos for playlist mode. Empty for `single` mode.
    #[serde(default)]
    pub videos: Vec<PublicationVideoRow>,
    /// Per-publication override for the "Like & Subscribe!" overlay.
    /// `null` means the publication inherits the global setting; `true`
    /// or `false` is an explicit override.
    #[serde(default)]
    pub like_subscribe_overlay: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublicationList {
    pub items: Vec<PublicationRow>,
}

// --- OAuth: start ---------------------------------------------------------

#[utoipa::path(
    get,
    path = "/integrations/youtube/oauth/start",
    tag = "integrations",
    responses(
        (status = 200, description = "Consent URL to navigate to", body = OauthStartResponse),
        (status = 401, description = "Unauthenticated"),
        (status = 503, description = "YouTube integration is not configured")
    ),
    security(("bearer" = []))
)]
pub async fn youtube_oauth_start(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<OauthStartResponse>> {
    let cfg = state.config();
    if cfg.youtube_client_id.trim().is_empty() || cfg.youtube_client_secret.trim().is_empty() {
        return Err(Error::Config("youtube oauth not configured".into()).into());
    }

    let token = random_state();
    persist_oauth_state(&state, &user.id, &token).await?;
    let url = oauth::build_consent_url(
        &cfg.youtube_client_id,
        &cfg.youtube_redirect_uri,
        &token,
        oauth::SCOPES,
    );
    Ok(Json(OauthStartResponse { url }))
}

// --- OAuth: callback ------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct OauthCallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[utoipa::path(
    get,
    path = "/integrations/youtube/oauth/callback",
    tag = "integrations",
    params(
        ("code" = Option<String>, Query, description = "Auth code from Google"),
        ("state" = Option<String>, Query, description = "Anti-CSRF state token issued by /start"),
        ("error" = Option<String>, Query, description = "Set when the user denied consent")
    ),
    responses(
        (status = 302, description = "Redirects back to the SPA settings page")
    )
)]
pub async fn youtube_oauth_callback(
    State(state): State<AppState>,
    Query(q): Query<OauthCallbackQuery>,
) -> Response {
    // Always bounce back to the SPA — the UI flashes a toast based on the
    // query string. Errors are encoded so the user sees what happened.
    let cfg = state.config();
    let dest = match handle_callback(&state, q).await {
        Ok(()) => format!("{}?connected=youtube", cfg.youtube_post_connect_redirect),
        Err(e) => {
            tracing::warn!(error = %e, "youtube oauth callback failed");
            format!(
                "{}?connected=youtube&error={}",
                cfg.youtube_post_connect_redirect,
                form_urlencoded::byte_serialize(e.to_string().as_bytes()).collect::<String>(),
            )
        }
    };
    Redirect::to(&dest).into_response()
}

async fn handle_callback(state: &AppState, q: OauthCallbackQuery) -> Result<()> {
    if let Some(err) = q.error.as_deref() {
        return Err(Error::Validation(format!("user denied consent: {err}")));
    }
    let code = q
        .code
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Validation("missing `code`".into()))?;
    let state_token = q
        .state
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Validation("missing `state`".into()))?;

    let user_id = consume_oauth_state(state, &state_token).await?;
    let cfg = state.config();
    let tokens = oauth::exchange_code(
        &cfg.youtube_client_id,
        &cfg.youtube_client_secret,
        &cfg.youtube_redirect_uri,
        &code,
    )
    .await?;
    let refresh = tokens.refresh_token.ok_or_else(|| {
        // Should never happen with `prompt=consent`, but degrade loudly.
        Error::Upstream(
            "Google did not issue a refresh token; please disconnect this app at \
             https://myaccount.google.com/permissions and try again"
                .into(),
        )
    })?;
    let channel = oauth::fetch_channel(&tokens.access_token).await?;

    let enc = encrypt::encrypt(&refresh, cfg.password_pepper.as_bytes())?;
    persist_account(state, &user_id, &channel.id, &channel.title, &enc).await?;
    Ok(())
}

// --- Account introspection / disconnect ----------------------------------

#[utoipa::path(
    get,
    path = "/integrations/youtube/account",
    tag = "integrations",
    responses(
        (status = 200, description = "Whether the calling user has YouTube connected", body = YoutubeAccountStatus),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn youtube_account_status(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<YoutubeAccountStatus>> {
    let row = load_account(&state, &user.id).await?;
    Ok(Json(match row {
        Some(r) => YoutubeAccountStatus {
            connected: true,
            channel_title: Some(r.channel_title),
            connected_at: Some(r.connected_at),
        },
        None => YoutubeAccountStatus {
            connected: false,
            channel_title: None,
            connected_at: None,
        },
    }))
}

#[utoipa::path(
    delete,
    path = "/integrations/youtube/account",
    tag = "integrations",
    responses(
        (status = 204, description = "Disconnected (revocation is best-effort)"),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn youtube_account_disconnect(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<StatusCode> {
    let cfg = state.config();
    let row = load_account(&state, &user.id).await?;
    if let Some(row) = row {
        // Best-effort revoke; log on failure but always delete the local row.
        if let Ok(token) = encrypt::decrypt(&row.refresh_token_enc, cfg.password_pepper.as_bytes())
        {
            if let Err(e) = oauth::revoke(&token).await {
                tracing::warn!(error = %e, "youtube revoke failed; deleting local row anyway");
            }
        }
    }
    state
        .db()
        .inner()
        .query(format!(
            "DELETE youtube_account WHERE owner = user:`{uid}`",
            uid = user.id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("yt disconnect: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt disconnect: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Publishing ----------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/publish/youtube",
    tag = "integrations",
    params(("id" = String, Path, description = "Audiobook id")),
    request_body = PublishYoutubeRequest,
    responses(
        (status = 202, description = "Publish queued; poll /audiobook/:id/publications", body = PublishYoutubeResponse),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Audiobook not found"),
        (status = 409, description = "Language not narrated yet, or YouTube not connected")
    ),
    security(("bearer" = []))
)]
pub async fn publish_youtube(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    axum::extract::Path(audiobook_id): axum::extract::Path<String>,
    Json(body): Json<PublishYoutubeRequest>,
) -> ApiResult<(StatusCode, Json<PublishYoutubeResponse>)> {
    let language = body.language.trim();
    if language.is_empty() {
        return Err(Error::Validation("language is required".into()).into());
    }
    let privacy = body
        .privacy_status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("private")
        .to_string();
    if !matches!(privacy.as_str(), "private" | "unlisted" | "public") {
        return Err(
            Error::Validation("privacy_status must be private/unlisted/public".into()).into(),
        );
    }
    let mut mode = body
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("single")
        .to_string();
    if !matches!(mode.as_str(), "single" | "playlist") {
        return Err(Error::Validation("mode must be single or playlist".into()).into());
    }
    let review = body.review.unwrap_or(false);
    let animate = body.animate.unwrap_or(false);

    // Ownership + readiness checks.
    assert_owner(&state, &audiobook_id, &user.id).await?;

    // Shorts always upload as a single vertical clip — silently
    // override the request rather than error out so a stale UI or
    // older client can't accidentally land a Short in playlist mode.
    if load_audiobook_is_short(&state, &audiobook_id).await? {
        mode = "single".to_string();
    }
    if !language_ready_for_publish(&state, &audiobook_id, language).await? {
        return Err(
            Error::Conflict(format!("language `{language}` is not fully narrated yet")).into(),
        );
    }
    // Review-only runs don't talk to Google, but a real publish does. Skip
    // the connected-account check if the user only wants a preview — they
    // may not have hooked up YouTube yet.
    if !review && load_account(&state, &user.id).await?.is_none() {
        return Err(Error::Conflict("connect a YouTube channel first".into()).into());
    }
    // animate=true requires every chapter to have its companion MP4
    // already on disk. We could enqueue the animate job from here, but
    // that would surprise the user — better to fail loudly so they
    // POST /animate first and watch the progress.
    if animate {
        if let Some(missing) = first_missing_animation(&state, &audiobook_id, language).await? {
            return Err(Error::Conflict(format!(
                "animation not ready: missing {} (run POST /audiobook/{}/animate first)",
                missing.display(),
                audiobook_id,
            ))
            .into());
        }
    }

    let publication_id = upsert_publication(
        &state,
        &audiobook_id,
        language,
        &privacy,
        &mode,
        review,
        body.like_subscribe_overlay,
    )
    .await?;

    let mut payload = serde_json::json!({
        "publication_id": publication_id,
        "privacy_status": privacy,
        "mode": mode,
        "review": review,
        "animate": animate,
    });
    if let Some(desc) = body.description.as_deref().filter(|s| !s.trim().is_empty()) {
        payload["description"] = serde_json::json!(desc);
    }

    let job_id = state
        .jobs()
        .enqueue(
            EnqueueRequest::new(JobKind::PublishYoutube)
                .with_user(user.id.clone())
                .with_audiobook(AudiobookId(audiobook_id.clone()))
                .with_language(language.to_string())
                .with_payload(payload)
                // Network-bound but each attempt re-runs ffmpeg too, which is
                // the expensive part. 3 attempts buys two retries against a
                // transient network blip without burning Google's per-day
                // quota (each attempt is one upload).
                .with_max_attempts(3),
        )
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(PublishYoutubeResponse {
            job_id: job_id.0,
            publication_id,
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/audiobook/{id}/publications",
    tag = "integrations",
    params(("id" = String, Path, description = "Audiobook id")),
    responses(
        (status = 200, description = "Every publication ever queued for this audiobook", body = PublicationList),
        (status = 404, description = "Audiobook not found")
    ),
    security(("bearer" = []))
)]
pub async fn list_publications(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    axum::extract::Path(audiobook_id): axum::extract::Path<String>,
) -> ApiResult<Json<PublicationList>> {
    assert_owner(&state, &audiobook_id, &user.id).await?;

    #[derive(Debug, Deserialize)]
    struct Row {
        id: Thing,
        language: String,
        privacy_status: String,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        review: Option<bool>,
        #[serde(default)]
        preview_ready_at: Option<DateTime<Utc>>,
        #[serde(default)]
        video_id: Option<String>,
        #[serde(default)]
        video_url: Option<String>,
        #[serde(default)]
        playlist_id: Option<String>,
        #[serde(default)]
        playlist_url: Option<String>,
        #[serde(default)]
        published_at: Option<DateTime<Utc>>,
        #[serde(default)]
        last_error: Option<String>,
        #[serde(default)]
        like_subscribe_overlay: Option<bool>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM youtube_publication \
             WHERE audiobook = audiobook:`{audiobook_id}` \
             ORDER BY created_at DESC"
        ))
        .await
        .map_err(|e| Error::Database(format!("publications list: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("publications list (decode): {e}")))?;

    #[derive(Debug, Deserialize)]
    struct VideoRow {
        publication: Thing,
        chapter_number: i64,
        title: String,
        #[serde(default)]
        video_id: Option<String>,
        #[serde(default)]
        video_url: Option<String>,
        #[serde(default)]
        published_at: Option<DateTime<Utc>>,
        #[serde(default)]
        last_error: Option<String>,
    }
    // One round-trip for every chapter video on this audiobook; cheaper
    // than N+1 even at a few publication rows since the table is tiny.
    let videos: Vec<VideoRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT publication, chapter_number, title, video_id, video_url, \
                    published_at, last_error \
             FROM youtube_publication_video \
             WHERE publication.audiobook = audiobook:`{audiobook_id}` \
             ORDER BY chapter_number ASC"
        ))
        .await
        .map_err(|e| Error::Database(format!("publication videos: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("publication videos (decode): {e}")))?;

    let mut by_pub: std::collections::HashMap<String, Vec<PublicationVideoRow>> =
        std::collections::HashMap::new();
    for v in videos {
        by_pub
            .entry(v.publication.id.to_raw())
            .or_default()
            .push(PublicationVideoRow {
                chapter_number: v.chapter_number,
                title: v.title,
                video_id: v.video_id,
                video_url: v.video_url,
                published_at: v.published_at,
                last_error: v.last_error,
            });
    }

    Ok(Json(PublicationList {
        items: rows
            .into_iter()
            .map(|r| {
                let id = r.id.id.to_raw();
                let videos = by_pub.remove(&id).unwrap_or_default();
                PublicationRow {
                    id,
                    language: r.language,
                    privacy_status: r.privacy_status,
                    mode: r.mode.unwrap_or_else(|| "single".to_string()),
                    review: r.review.unwrap_or(false),
                    preview_ready_at: r.preview_ready_at,
                    video_id: r.video_id,
                    video_url: r.video_url,
                    playlist_id: r.playlist_id,
                    playlist_url: r.playlist_url,
                    published_at: r.published_at,
                    last_error: r.last_error,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                    videos,
                    like_subscribe_overlay: r.like_subscribe_overlay,
                }
            })
            .collect(),
    }))
}

// --- Review: approve / cancel / preview stream ---------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct ApprovePublicationResponse {
    /// New job id that will perform the actual upload.
    pub job_id: String,
    pub publication_id: String,
}

/// Approve a previewed publication: clear the review flag and enqueue the
/// real publish job (which now skips the encode step because the MP4(s)
/// already exist on disk).
#[utoipa::path(
    post,
    path = "/audiobook/{id}/publications/{pid}/approve",
    tag = "integrations",
    params(
        ("id" = String, Path, description = "Audiobook id"),
        ("pid" = String, Path, description = "Publication id")
    ),
    responses(
        (status = 202, description = "Upload queued", body = ApprovePublicationResponse),
        (status = 404, description = "Publication not found"),
        (status = 409, description = "Publication is not in preview state, or YouTube not connected")
    ),
    security(("bearer" = []))
)]
pub async fn approve_publication(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    axum::extract::Path((audiobook_id, publication_id)): axum::extract::Path<(String, String)>,
) -> ApiResult<(StatusCode, Json<ApprovePublicationResponse>)> {
    assert_owner(&state, &audiobook_id, &user.id).await?;
    if load_account(&state, &user.id).await?.is_none() {
        return Err(Error::Conflict("connect a YouTube channel first".into()).into());
    }
    let pub_row = load_publication(&state, &audiobook_id, &publication_id).await?;
    if !pub_row.review || pub_row.preview_ready_at.is_none() {
        return Err(Error::Conflict("publication is not awaiting review approval".into()).into());
    }
    if pub_row.published_at.is_some() {
        return Err(Error::Conflict("publication already published".into()).into());
    }

    state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_publication:`{publication_id}` SET \
                review = false, \
                preview_ready_at = NONE, \
                last_error = NONE, \
                updated_at = time::now()"
        ))
        .await
        .map_err(|e| Error::Database(format!("yt approve: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt approve: {e}")))?;

    let payload = serde_json::json!({
        "publication_id": publication_id,
        "privacy_status": pub_row.privacy_status,
        "mode": pub_row.mode,
        "review": false,
    });
    let job_id = state
        .jobs()
        .enqueue(
            EnqueueRequest::new(JobKind::PublishYoutube)
                .with_user(user.id.clone())
                .with_audiobook(AudiobookId(audiobook_id.clone()))
                .with_language(pub_row.language)
                .with_payload(payload)
                .with_max_attempts(3),
        )
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(ApprovePublicationResponse {
            job_id: job_id.0,
            publication_id,
        }),
    ))
}

/// Discard a previewed publication: delete intermediate MP4 files on disk
/// and reset the publication's review state so the next publish runs fresh.
/// The publication row itself is kept (for any prior video/playlist links).
#[utoipa::path(
    post,
    path = "/audiobook/{id}/publications/{pid}/cancel",
    tag = "integrations",
    params(
        ("id" = String, Path, description = "Audiobook id"),
        ("pid" = String, Path, description = "Publication id")
    ),
    responses(
        (status = 204, description = "Preview discarded"),
        (status = 404, description = "Publication not found"),
        (status = 409, description = "Publication is not in preview state")
    ),
    security(("bearer" = []))
)]
pub async fn cancel_publication(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    axum::extract::Path((audiobook_id, publication_id)): axum::extract::Path<(String, String)>,
) -> ApiResult<StatusCode> {
    assert_owner(&state, &audiobook_id, &user.id).await?;
    let pub_row = load_publication(&state, &audiobook_id, &publication_id).await?;
    if !pub_row.review {
        return Err(Error::Conflict("publication is not in review state".into()).into());
    }

    // Delete the MP4 files; failures are warnings, not errors — the worst
    // case is a stale file that the next encode will overwrite anyway.
    for path in preview_mp4_paths(&state, &audiobook_id, &pub_row.language, &pub_row.mode) {
        if path.exists() {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!(error = %e, ?path, "yt cancel: remove preview mp4 failed");
            }
        }
    }

    state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_publication:`{publication_id}` SET \
                review = false, \
                preview_ready_at = NONE, \
                last_error = NONE, \
                updated_at = time::now()"
        ))
        .await
        .map_err(|e| Error::Database(format!("yt cancel: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt cancel: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Stream the encoded MP4 for a previewed publication. Accepts an
/// `Authorization: Bearer …` header *or* `?access_token=…` so the
/// browser's `<video>` tag can play it directly. Honours the `Range`
/// header so seeking works.
#[derive(Debug, Deserialize)]
pub struct PreviewQuery {
    #[serde(default)]
    pub access_token: Option<String>,
    /// 1-based chapter number. Required for playlist-mode publications;
    /// ignored for single-mode.
    #[serde(default)]
    pub chapter: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/audiobook/{id}/publications/{pid}/preview",
    tag = "integrations",
    params(
        ("id" = String, Path, description = "Audiobook id"),
        ("pid" = String, Path, description = "Publication id"),
        ("chapter" = Option<u32>, Query, description = "Chapter number (playlist mode)")
    ),
    responses(
        (status = 200, description = "MP4 bytes", content_type = "video/mp4"),
        (status = 206, description = "Partial MP4 (range request)", content_type = "video/mp4"),
        (status = 404, description = "Preview not available")
    ),
    security(("bearer" = []))
)]
pub async fn preview_publication(
    State(state): State<AppState>,
    auth_header: Option<Authenticated>,
    axum::extract::Path((audiobook_id, publication_id)): axum::extract::Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<PreviewQuery>,
    headers: axum::http::HeaderMap,
) -> ApiResult<axum::response::Response> {
    let user_id = match auth_header {
        Some(Authenticated(u)) => u.id,
        None => {
            let token = q.access_token.as_deref().ok_or(Error::Unauthorized)?;
            crate::auth::tokens::verify_access_token(token, &state.config().jwt_secret)?.sub
        }
    };
    assert_owner(&state, &audiobook_id, &user_id).await?;
    let pub_row = load_publication(&state, &audiobook_id, &publication_id).await?;

    let path = match pub_row.mode.as_str() {
        "playlist" => {
            let ch = q
                .chapter
                .ok_or_else(|| Error::Validation("chapter is required for playlist mode".into()))?;
            state
                .config()
                .storage_path
                .join(&audiobook_id)
                .join(&pub_row.language)
                .join(format!("youtube-ch-{ch}.mp4"))
        }
        _ => state
            .config()
            .storage_path
            .join(&audiobook_id)
            .join(&pub_row.language)
            .join("youtube.mp4"),
    };

    let total_len = match tokio::fs::metadata(&path).await {
        Ok(m) => m.len(),
        Err(_) => {
            return Err(Error::NotFound {
                resource: format!("preview for publication:{publication_id}"),
            }
            .into())
        }
    };

    serve_mp4_with_range(&path, total_len, headers.get(axum::http::header::RANGE)).await
}

async fn serve_mp4_with_range(
    path: &std::path::Path,
    total: u64,
    range_header: Option<&axum::http::HeaderValue>,
) -> ApiResult<axum::response::Response> {
    use axum::body::Body;
    use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
    use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

    let range = range_header
        .and_then(|v| v.to_str().ok())
        .and_then(parse_byte_range);
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("open preview: {e}")))?;

    let (start, end, status) = match range {
        Some((s, e)) if s < total => {
            let end = e.unwrap_or(total - 1).min(total - 1);
            (s, end, StatusCode::PARTIAL_CONTENT)
        }
        _ => (0u64, total - 1, StatusCode::OK),
    };
    let len = end - start + 1;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    if status == StatusCode::PARTIAL_CONTENT {
        if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")) {
            headers.insert(header::CONTENT_RANGE, v);
        }
    }

    file.seek(SeekFrom::Start(start))
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("seek preview: {e}")))?;
    // Buffer in memory; previews are short and bounded by chapter length.
    // For very long single-mode books this could be replaced with a
    // streaming reader that respects `len`, but the simplicity is worth
    // it for now.
    let mut buf = vec![0u8; len as usize];
    file.read_exact(&mut buf)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("read preview: {e}")))?;
    Ok((status, headers, Body::from(buf)).into_response())
}

fn parse_byte_range(raw: &str) -> Option<(u64, Option<u64>)> {
    // Only the single-range "bytes=start-[end]" form is supported, which is
    // what every modern browser sends for `<video>` seeking.
    let after = raw.strip_prefix("bytes=")?;
    let mut parts = after.splitn(2, '-');
    let start: u64 = parts.next()?.trim().parse().ok()?;
    let end = parts.next().and_then(|s| s.trim().parse::<u64>().ok());
    Some((start, end))
}

fn preview_mp4_paths(
    state: &AppState,
    audiobook_id: &str,
    language: &str,
    mode: &str,
) -> Vec<std::path::PathBuf> {
    let dir = state
        .config()
        .storage_path
        .join(audiobook_id)
        .join(language);
    if mode == "playlist" {
        // Walk the dir and collect every chapter MP4 we find. Doing it via
        // glob keeps us from having to look up the chapter list again.
        match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("youtube-ch-") && n.ends_with(".mp4"))
                        .unwrap_or(false)
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        vec![dir.join("youtube.mp4")]
    }
}

#[derive(Debug, Deserialize)]
struct PublicationFull {
    language: String,
    mode: String,
    privacy_status: String,
    #[serde(default)]
    review: Option<bool>,
    #[serde(default)]
    preview_ready_at: Option<DateTime<Utc>>,
    #[serde(default)]
    published_at: Option<DateTime<Utc>>,
}

struct LoadedPublication {
    language: String,
    mode: String,
    privacy_status: String,
    review: bool,
    preview_ready_at: Option<DateTime<Utc>>,
    published_at: Option<DateTime<Utc>>,
}

async fn load_publication(
    state: &AppState,
    audiobook_id: &str,
    publication_id: &str,
) -> Result<LoadedPublication> {
    #[derive(Debug, Deserialize)]
    struct Row {
        audiobook: Thing,
        #[serde(flatten)]
        inner: PublicationFull,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT audiobook, language, mode, privacy_status, review, \
                    preview_ready_at, published_at \
             FROM youtube_publication:`{publication_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("yt pub load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt pub load (decode): {e}")))?;
    let row = rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("publication:{publication_id}"),
    })?;
    if row.audiobook.id.to_raw() != audiobook_id {
        // Don't leak existence of unrelated publications.
        return Err(Error::NotFound {
            resource: format!("publication:{publication_id}"),
        });
    }
    let inner = row.inner;
    Ok(LoadedPublication {
        language: inner.language,
        mode: inner.mode,
        privacy_status: inner.privacy_status,
        review: inner.review.unwrap_or(false),
        preview_ready_at: inner.preview_ready_at,
        published_at: inner.published_at,
    })
}

// --- Internal helpers ----------------------------------------------------

#[derive(Debug, Deserialize)]
struct DbAccount {
    channel_title: String,
    refresh_token_enc: String,
    connected_at: DateTime<Utc>,
}

async fn load_account(state: &AppState, user_id: &UserId) -> Result<Option<DbAccount>> {
    let rows: Vec<DbAccount> = state
        .db()
        .inner()
        .query(format!(
            "SELECT channel_title, refresh_token_enc, connected_at \
             FROM youtube_account WHERE owner = user:`{}` LIMIT 1",
            user_id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("yt account load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt account load (decode): {e}")))?;
    Ok(rows.into_iter().next())
}

async fn persist_account(
    state: &AppState,
    user_id: &UserId,
    channel_id: &str,
    channel_title: &str,
    refresh_enc: &str,
) -> Result<()> {
    // Delete-then-create so the unique `owner` index never trips when a user
    // reconnects with a different channel.
    let scopes_json = serde_json::to_string(oauth::SCOPES).unwrap_or_else(|_| "[]".to_string());
    let sql = format!(
        r#"DELETE youtube_account WHERE owner = user:`{uid}`;
           CREATE youtube_account CONTENT {{
             owner: user:`{uid}`,
             channel_id: $cid,
             channel_title: $ctitle,
             refresh_token_enc: $enc,
             scopes: {scopes_json}
           }}"#,
        uid = user_id.0,
    );
    state
        .db()
        .inner()
        .query(sql)
        .bind(("cid", channel_id.to_string()))
        .bind(("ctitle", channel_title.to_string()))
        .bind(("enc", refresh_enc.to_string()))
        .await
        .map_err(|e| Error::Database(format!("yt account upsert: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt account upsert: {e}")))?;
    Ok(())
}

async fn persist_oauth_state(state: &AppState, user_id: &UserId, token: &str) -> Result<()> {
    // 10-minute window — matches Google's own token grace and is plenty for a
    // real user clicking through the consent screen.
    let sql = format!(
        r#"CREATE oauth_state CONTENT {{
            user: user:`{uid}`,
            provider: "youtube",
            state: $state,
            expires_at: time::now() + 10m
        }}"#,
        uid = user_id.0,
    );
    state
        .db()
        .inner()
        .query(sql)
        .bind(("state", token.to_string()))
        .await
        .map_err(|e| Error::Database(format!("oauth_state insert: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("oauth_state insert: {e}")))?;
    Ok(())
}

async fn consume_oauth_state(state: &AppState, token: &str) -> Result<UserId> {
    #[derive(Debug, Deserialize)]
    struct Row {
        id: Thing,
        user: Thing,
        expires_at: DateTime<Utc>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query("SELECT id, user, expires_at FROM oauth_state WHERE state = $s LIMIT 1")
        .bind(("s", token.to_string()))
        .await
        .map_err(|e| Error::Database(format!("oauth_state lookup: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("oauth_state lookup (decode): {e}")))?;
    let row = rows.into_iter().next().ok_or(Error::Unauthorized)?;
    if row.expires_at < Utc::now() {
        return Err(Error::Unauthorized);
    }
    // One-shot — consume it before doing the network round-trip so a slow
    // reply can't be replayed.
    let raw_id = row.id.id.to_raw();
    state
        .db()
        .inner()
        .query(format!("DELETE oauth_state:`{raw_id}`"))
        .await
        .map_err(|e| Error::Database(format!("oauth_state delete: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("oauth_state delete: {e}")))?;
    Ok(UserId(row.user.id.to_raw()))
}

async fn assert_owner(state: &AppState, audiobook_id: &str, user: &UserId) -> Result<()> {
    #[derive(Debug, Deserialize)]
    struct Row {
        owner: Thing,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!("SELECT owner FROM audiobook:`{audiobook_id}`"))
        .await
        .map_err(|e| Error::Database(format!("publish owner: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("publish owner (decode): {e}")))?;
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

/// Read the `is_short` flag off `audiobook:<id>`. Defaults to `false`
/// when the field is absent (older rows pre-migration 0031). Used by
/// the publish handler to clamp Shorts to single-video mode regardless
/// of what the request asked for.
async fn load_audiobook_is_short(state: &AppState, audiobook_id: &str) -> Result<bool> {
    #[derive(Debug, Deserialize)]
    struct Row {
        #[serde(default)]
        is_short: Option<bool>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!("SELECT is_short FROM audiobook:`{audiobook_id}`"))
        .await
        .map_err(|e| Error::Database(format!("yt pub is_short: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt pub is_short (decode): {e}")))?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.is_short)
        .unwrap_or(false))
}

async fn language_ready_for_publish(
    state: &AppState,
    audiobook_id: &str,
    language: &str,
) -> Result<bool> {
    #[derive(Debug, Deserialize)]
    struct Row {
        total: i64,
        ready: i64,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT \
                count() AS total, \
                count(status = \"audio_ready\") AS ready \
             FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang \
             GROUP ALL"
        ))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("publish readiness: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("publish readiness (decode): {e}")))?;
    let row = rows.into_iter().next();
    Ok(matches!(row, Some(r) if r.total > 0 && r.total == r.ready))
}

/// Walk every chapter in `language` and return the first
/// `ch-N.video.mp4` that doesn't exist on disk, or `None` if all are
/// present. Used to gate `animate=true` publish requests.
async fn first_missing_animation(
    state: &AppState,
    audiobook_id: &str,
    language: &str,
) -> Result<Option<std::path::PathBuf>> {
    #[derive(Debug, Deserialize)]
    struct Row {
        number: i64,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT number FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang \
             ORDER BY number ASC"
        ))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("animate readiness: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("animate readiness (decode): {e}")))?;
    let dir = state
        .config()
        .storage_path
        .join(audiobook_id)
        .join(language);
    for r in rows {
        let p = dir.join(format!("ch-{}.video.mp4", r.number));
        if !p.exists() {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

async fn upsert_publication(
    state: &AppState,
    audiobook_id: &str,
    language: &str,
    privacy: &str,
    mode: &str,
    review: bool,
    // `None` clears any existing override (publication will inherit the
    // global setting); `Some(true)`/`Some(false)` writes an explicit
    // override that takes precedence over the global.
    like_subscribe_overlay: Option<bool>,
) -> Result<String> {
    // Fast path: existing row → update + return its id. Switching mode on
    // an existing publication clears any previously stored single-video id
    // / playlist id so the worker doesn't reuse a stale link.
    #[derive(Debug, Deserialize)]
    struct Row {
        id: Thing,
        #[serde(default)]
        mode: Option<String>,
    }
    let existing: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, mode FROM youtube_publication \
             WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang LIMIT 1"
        ))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("yt pub lookup: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt pub lookup (decode): {e}")))?;

    if let Some(row) = existing.into_iter().next() {
        let raw = row.id.id.to_raw();
        let mode_changed = row.mode.as_deref().unwrap_or("single") != mode;
        let extra_clear = if mode_changed {
            // Wipe state from the previous mode so it doesn't bleed into
            // the new run (e.g. previously single → now playlist should
            // not reuse the old single-video id).
            ", video_id = NONE, video_url = NONE, playlist_id = NONE, \
              playlist_url = NONE, published_at = NONE"
        } else {
            ""
        };
        state
            .db()
            .inner()
            .query(format!(
                "UPDATE youtube_publication:`{raw}` SET \
                    privacy_status = $p, \
                    mode = $m, \
                    review = $r, \
                    like_subscribe_overlay = $ls, \
                    preview_ready_at = NONE, \
                    last_error = NONE, \
                    updated_at = time::now() \
                    {extra_clear}"
            ))
            .bind(("p", privacy.to_string()))
            .bind(("m", mode.to_string()))
            .bind(("r", review))
            .bind(("ls", like_subscribe_overlay))
            .await
            .map_err(|e| Error::Database(format!("yt pub update: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("yt pub update: {e}")))?;
        if mode_changed {
            // Per-chapter video rows belong to the previous run; drop
            // them so the new playlist run starts from a clean slate.
            state
                .db()
                .inner()
                .query(format!(
                    "DELETE youtube_publication_video \
                       WHERE publication = youtube_publication:`{raw}`"
                ))
                .await
                .map_err(|e| Error::Database(format!("yt pub video clear: {e}")))?
                .check()
                .map_err(|e| Error::Database(format!("yt pub video clear: {e}")))?;
        }
        return Ok(raw);
    }

    let id = uuid::Uuid::new_v4().simple().to_string();
    state
        .db()
        .inner()
        .query(format!(
            r#"CREATE youtube_publication:`{id}` CONTENT {{
                audiobook: audiobook:`{audiobook_id}`,
                language: $lang,
                privacy_status: $p,
                mode: $m,
                review: $r,
                like_subscribe_overlay: $ls
            }}"#
        ))
        .bind(("lang", language.to_string()))
        .bind(("p", privacy.to_string()))
        .bind(("m", mode.to_string()))
        .bind(("r", review))
        .bind(("ls", like_subscribe_overlay))
        .await
        .map_err(|e| Error::Database(format!("yt pub create: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt pub create: {e}")))?;
    Ok(id)
}

fn random_state() -> String {
    let mut buf = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}
