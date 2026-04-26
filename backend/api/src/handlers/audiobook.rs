//! Audiobook CRUD + content-generation triggers.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use listenai_core::domain::{AudiobookLength, AudiobookStatus, ChapterStatus, JobKind};
use listenai_core::id::{AudiobookId, ChapterId, UserId};
use listenai_core::{Error, Result};
use listenai_jobs::repo::EnqueueRequest;
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::generation::{audio as audio_gen, chapter as chapter_gen, outline as outline_gen};
use crate::idempotency::{self, IdempotencyKey};
use crate::state::AppState;
use tracing::warn;

// -------------------------------------------------------------------------
// DTOs
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateAudiobookRequest {
    #[validate(length(min = 3, max = 500))]
    pub topic: String,
    pub length: AudiobookLength,
    #[validate(length(max = 40))]
    pub genre: Option<String>,
    /// Optional voice id from `/voices`. If omitted, the TTS layer falls
    /// back to the configured `xai_default_voice`.
    #[validate(length(min = 1, max = 64))]
    pub voice_id: Option<String>,
    /// Optional pre-generated cover artwork. Raw base64 (no `data:` prefix);
    /// produced by `POST /cover-art/preview`. Persisted to disk and served
    /// later via `GET /audiobook/:id/cover`.
    pub cover_image_base64: Option<String>,
    /// BCP-47 language code, e.g. `"en"`, `"nl"`, `"de"`. Drives both LLM
    /// content generation and TTS narration. Defaults to `"en"`.
    #[validate(length(min = 2, max = 8))]
    pub language: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookSummary {
    pub id: AudiobookId,
    pub title: String,
    pub topic: String,
    pub genre: Option<String>,
    pub length: AudiobookLength,
    pub status: AudiobookStatus,
    /// `true` when a cover image has been generated and is available at
    /// `GET /audiobook/:id/cover`.
    pub has_cover: bool,
    /// BCP-47 code for the audiobook's *primary* (originally generated)
    /// language. Stays stable; `available_languages` grows as translations
    /// are added.
    pub language: String,
    /// Every language this audiobook has chapters in (always includes the
    /// primary language). Drives the language switcher on the detail/player
    /// pages.
    pub available_languages: Vec<String>,
    /// Voice picked for narration (`None` when the server default is used).
    /// Frontend resolves the human-readable name via `/voices`.
    pub voice_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookDetail {
    #[serde(flatten)]
    pub summary: AudiobookSummary,
    pub chapters: Vec<ChapterSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ChapterSummary {
    pub id: ChapterId,
    pub number: u32,
    pub title: String,
    pub synopsis: Option<String>,
    pub target_words: Option<u32>,
    pub body_md: Option<String>,
    pub status: ChapterStatus,
    /// WAV duration in milliseconds, populated once the chapter has been
    /// narrated. The player uses this to render a whole-book progress bar.
    pub duration_ms: Option<u64>,
    /// Which language version this chapter belongs to.
    pub language: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookList {
    pub items: Vec<AudiobookSummary>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateAudiobookRequest {
    #[validate(length(min = 1, max = 200))]
    pub title: Option<String>,
    #[validate(length(max = 40))]
    pub genre: Option<String>,
    /// Pass `Some("eve")` to change the narrator. Re-narrate the audiobook
    /// after changing this for the new voice to take effect on existing
    /// audio files.
    #[validate(length(min = 1, max = 64))]
    pub voice_id: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateChapterRequest {
    #[validate(length(min = 1, max = 200))]
    pub title: Option<String>,
    #[validate(length(max = 2_000))]
    pub synopsis: Option<String>,
    pub body_md: Option<String>,
}

// -------------------------------------------------------------------------
// Row types (private)
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DbAudiobook {
    id: Thing,
    owner: Thing,
    title: String,
    topic: String,
    genre: Option<String>,
    length: String,
    status: String,
    cover_path: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    primary_voice: Option<Thing>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct DbChapter {
    id: Thing,
    number: i64,
    title: String,
    synopsis: Option<String>,
    target_words: Option<i64>,
    body_md: Option<String>,
    status: String,
    duration_ms: Option<i64>,
    #[serde(default)]
    language: Option<String>,
}

impl DbAudiobook {
    fn to_summary(&self) -> Result<AudiobookSummary> {
        Ok(AudiobookSummary {
            id: AudiobookId(self.id.id.to_raw()),
            title: self.title.clone(),
            topic: self.topic.clone(),
            genre: self.genre.clone(),
            length: parse_length(&self.length)?,
            status: parse_status(&self.status)?,
            has_cover: self
                .cover_path
                .as_deref()
                .map(|p| !p.trim().is_empty())
                .unwrap_or(false),
            language: self
                .language
                .clone()
                .unwrap_or_else(|| "en".to_string()),
            // Filled in by `enrich_summary` once the chapter set is loaded.
            available_languages: Vec::new(),
            voice_id: self.primary_voice.as_ref().map(|t| t.id.to_raw()),
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }

    fn owner_id(&self) -> UserId {
        UserId(self.owner.id.to_raw())
    }
}

impl DbChapter {
    fn to_summary(&self) -> Result<ChapterSummary> {
        Ok(ChapterSummary {
            id: ChapterId(self.id.id.to_raw()),
            number: self.number as u32,
            title: self.title.clone(),
            synopsis: self.synopsis.clone(),
            target_words: self.target_words.map(|w| w as u32),
            body_md: self.body_md.clone(),
            status: parse_chapter_status(&self.status)?,
            duration_ms: self.duration_ms.map(|d| d.max(0) as u64),
            language: self
                .language
                .clone()
                .unwrap_or_else(|| "en".to_string()),
        })
    }
}

fn parse_length(s: &str) -> Result<AudiobookLength> {
    match s {
        "short" => Ok(AudiobookLength::Short),
        "medium" => Ok(AudiobookLength::Medium),
        "long" => Ok(AudiobookLength::Long),
        other => Err(Error::Database(format!("unknown length `{other}`"))),
    }
}

fn parse_status(s: &str) -> Result<AudiobookStatus> {
    Ok(match s {
        "draft" => AudiobookStatus::Draft,
        "outline_pending" => AudiobookStatus::OutlinePending,
        "outline_ready" => AudiobookStatus::OutlineReady,
        "chapters_running" => AudiobookStatus::ChaptersRunning,
        "text_ready" => AudiobookStatus::TextReady,
        "audio_ready" => AudiobookStatus::AudioReady,
        "failed" => AudiobookStatus::Failed,
        other => return Err(Error::Database(format!("unknown status `{other}`"))),
    })
}

fn parse_chapter_status(s: &str) -> Result<ChapterStatus> {
    Ok(match s {
        "pending" => ChapterStatus::Pending,
        "running" => ChapterStatus::Running,
        "text_ready" => ChapterStatus::TextReady,
        "audio_ready" => ChapterStatus::AudioReady,
        "failed" => ChapterStatus::Failed,
        other => return Err(Error::Database(format!("unknown chapter status `{other}`"))),
    })
}

fn length_to_str(l: AudiobookLength) -> &'static str {
    match l {
        AudiobookLength::Short => "short",
        AudiobookLength::Medium => "medium",
        AudiobookLength::Long => "long",
    }
}

// -------------------------------------------------------------------------
// POST /audiobook  — creates draft + runs outline synchronously
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook",
    tag = "audiobook",
    request_body = CreateAudiobookRequest,
    responses(
        (status = 200, description = "Outline ready", body = AudiobookDetail),
        (status = 400, description = "Validation failed"),
        (status = 401, description = "Unauthenticated"),
        (status = 502, description = "Upstream LLM error")
    ),
    security(("bearer" = []))
)]
pub async fn create(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Json(body): Json<CreateAudiobookRequest>,
) -> ApiResult<Json<AudiobookDetail>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    let id = Uuid::new_v4().simple().to_string();
    let genre = body.genre.clone().unwrap_or_default();

    // Pick + validate language. Default to "en" so old clients keep working.
    let language = body
        .language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("en")
        .to_string();
    if !is_supported_language(&language) {
        return Err(Error::Validation(format!(
            "unsupported language `{language}`"
        ))
        .into());
    }

    // Validate voice_id refers to an enabled voice — fail fast with a 400
    // rather than letting a bad pick surface as a TTS failure 30 s later.
    let voice_id = match body.voice_id.as_deref() {
        Some(v) if !v.trim().is_empty() => Some(assert_voice_enabled(&state, v.trim()).await?),
        _ => None,
    };

    let voice_clause = match &voice_id {
        Some(vid) => format!("primary_voice: voice:`{vid}`,"),
        None => String::new(),
    };
    let sql = format!(
        r#"CREATE audiobook:`{id}` CONTENT {{
            owner: user:`{user_id}`,
            title: "Untitled",
            topic: $topic,
            genre: $genre,
            length: $length,
            language: $language,
            {voice_clause}
            status: "draft"
        }}"#,
        user_id = user.id.0,
    );
    state
        .db()
        .inner()
        .query(sql)
        .bind(("topic", body.topic.trim().to_string()))
        .bind((
            "genre",
            if genre.is_empty() {
                None
            } else {
                Some(genre.clone())
            },
        ))
        .bind(("length", length_to_str(body.length).to_string()))
        .bind(("language", language.clone()))
        .await
        .map_err(|e| Error::Database(format!("create audiobook: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create audiobook: {e}")))?;

    // Persist cover bytes if the client included them. Failure here only
    // logs — it must not block outline generation.
    if let Some(b64) = body.cover_image_base64.as_deref() {
        if let Err(e) = persist_cover(&state, &id, b64).await {
            warn!(error = %e, audiobook_id = id, "create: cover persist failed");
        }
    }

    outline_gen::run(
        &state,
        &user.id,
        &id,
        &body.topic,
        body.length,
        if genre.is_empty() { "any" } else { &genre },
        &language,
    )
    .await?;

    Ok(Json(load_detail(&state, &id, &user.id, None).await?))
}

// -------------------------------------------------------------------------
// GET /audiobook  — list my audiobooks
// -------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/audiobook",
    tag = "audiobook",
    responses(
        (status = 200, description = "All audiobooks owned by the authed user", body = AudiobookList),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<AudiobookList>> {
    let rows: Vec<DbAudiobook> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM audiobook WHERE owner = user:`{}` ORDER BY created_at DESC",
            user.id.0,
        ))
        .await
        .map_err(|e| Error::Database(format!("list audiobooks: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list audiobooks (decode): {e}")))?;

    let items = rows
        .iter()
        .map(DbAudiobook::to_summary)
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(AudiobookList { items }))
}

// -------------------------------------------------------------------------
// GET /audiobook/:id
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GetOneQuery {
    /// Optional language filter; defaults to the audiobook's primary
    /// language. Only chapters in this language are returned.
    #[serde(default)]
    pub language: Option<String>,
}

#[utoipa::path(
    get,
    path = "/audiobook/{id}",
    tag = "audiobook",
    params(
        ("id" = String, Path, description = "Audiobook id"),
        ("language" = Option<String>, Query, description = "Language filter (default: audiobook primary)")
    ),
    responses(
        (status = 200, description = "Audiobook + chapters", body = AudiobookDetail),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn get_one(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<GetOneQuery>,
) -> ApiResult<Json<AudiobookDetail>> {
    Ok(Json(
        load_detail(&state, &id, &user.id, q.language.as_deref()).await?,
    ))
}

// -------------------------------------------------------------------------
// PATCH /audiobook/:id  — edit title or genre
// -------------------------------------------------------------------------

#[utoipa::path(
    patch,
    path = "/audiobook/{id}",
    tag = "audiobook",
    params(("id" = String, Path)),
    request_body = UpdateAudiobookRequest,
    responses(
        (status = 200, description = "Updated audiobook", body = AudiobookDetail),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn patch(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    Json(body): Json<UpdateAudiobookRequest>,
) -> ApiResult<Json<AudiobookDetail>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    assert_owner(&state, &id, &user.id).await?;

    if let Some(title) = body.title {
        state
            .db()
            .inner()
            .query(format!("UPDATE audiobook:`{id}` SET title = $t"))
            .bind(("t", title.trim().to_string()))
            .await
            .map_err(|e| Error::Database(format!("patch title: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch title: {e}")))?;
    }
    if let Some(genre) = body.genre {
        state
            .db()
            .inner()
            .query(format!("UPDATE audiobook:`{id}` SET genre = $g"))
            .bind(("g", Some(genre.trim().to_string())))
            .await
            .map_err(|e| Error::Database(format!("patch genre: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch genre: {e}")))?;
    }
    if let Some(voice) = body.voice_id {
        let trimmed = voice.trim();
        if trimmed.is_empty() {
            // Empty string clears the override; the TTS layer falls back to
            // `Config.xai_default_voice`.
            state
                .db()
                .inner()
                .query(format!("UPDATE audiobook:`{id}` SET primary_voice = NONE"))
                .await
                .map_err(|e| Error::Database(format!("clear voice: {e}")))?
                .check()
                .map_err(|e| Error::Database(format!("clear voice: {e}")))?;
        } else {
            let resolved = assert_voice_enabled(&state, trimmed).await?;
            state
                .db()
                .inner()
                .query(format!(
                    "UPDATE audiobook:`{id}` SET primary_voice = voice:`{resolved}`"
                ))
                .await
                .map_err(|e| Error::Database(format!("patch voice: {e}")))?
                .check()
                .map_err(|e| Error::Database(format!("patch voice: {e}")))?;
        }
    }

    Ok(Json(load_detail(&state, &id, &user.id, None).await?))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/cover  — (re)generate cover from topic + genre
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/cover",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "Cover regenerated", body = AudiobookDetail),
        (status = 404, description = "Not found"),
        (status = 502, description = "Upstream image-gen error")
    ),
    security(("bearer" = []))
)]
pub async fn regenerate_cover(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<Json<AudiobookDetail>> {
    assert_owner(&state, &id, &user.id).await?;
    let book = load_audiobook(&state, &id).await?;
    let bytes = crate::generation::cover::generate(
        &state,
        &book.topic,
        book.genre.as_deref(),
    )
    .await?;
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    let b64 = B64.encode(&bytes);
    persist_cover(&state, &id, &b64).await?;
    Ok(Json(load_detail(&state, &id, &user.id, None).await?))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/translate  — add a translated language version
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct TranslateRequest {
    /// Target language code (e.g. `"nl"`).
    #[validate(length(min = 2, max = 8))]
    pub target_language: String,
    /// Optional source language to translate from (defaults to the
    /// audiobook's primary language).
    #[validate(length(min = 2, max = 8))]
    pub source_language: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TranslateResponse {
    /// Id of the background job. Watch its status via the WebSocket or
    /// `GET /audiobook/:id/jobs`. Chapter rows in the target language land
    /// one at a time as the job progresses.
    pub job_id: String,
    pub target_language: String,
    pub source_language: String,
}

#[utoipa::path(
    post,
    path = "/audiobook/{id}/translate",
    tag = "audiobook",
    params(("id" = String, Path)),
    request_body = TranslateRequest,
    responses(
        (status = 202, description = "Translation queued; poll /audiobook/:id/jobs", body = TranslateResponse),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found"),
        (status = 409, description = "A translation to this language is already running")
    ),
    security(("bearer" = []))
)]
pub async fn translate(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    Json(body): Json<TranslateRequest>,
) -> ApiResult<(StatusCode, Json<TranslateResponse>)> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    if !is_supported_language(&body.target_language) {
        return Err(Error::Validation(format!(
            "unsupported language `{}`",
            body.target_language
        ))
        .into());
    }

    assert_owner(&state, &id, &user.id).await?;
    let book = load_audiobook(&state, &id).await?;
    let primary = book
        .language
        .clone()
        .unwrap_or_else(|| "en".to_string());
    let source = body
        .source_language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&primary)
        .to_string();

    if source == body.target_language {
        return Err(Error::Validation(
            "source and target language must differ".into(),
        )
        .into());
    }

    if has_live_translate_job(&state, &id, &body.target_language).await? {
        return Err(Error::Conflict(format!(
            "translation to `{}` already running",
            body.target_language
        ))
        .into());
    }

    let job_id = state
        .jobs()
        .enqueue(
            EnqueueRequest::new(JobKind::Translate)
                .with_user(user.id.clone())
                .with_audiobook(AudiobookId(id.clone()))
                .with_language(body.target_language.clone())
                .with_payload(serde_json::json!({
                    "source_language": source,
                }))
                .with_max_attempts(3),
        )
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(TranslateResponse {
            job_id: job_id.0,
            target_language: body.target_language,
            source_language: source,
        }),
    ))
}

async fn has_live_translate_job(
    state: &AppState,
    audiobook_id: &str,
    target: &str,
) -> Result<bool> {
    #[derive(Deserialize)]
    struct CountRow {
        count: i64,
    }
    let rows: Vec<CountRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM job \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND kind = \"translate\" \
               AND language = $lang \
               AND status IN [\"queued\", \"running\"] \
             GROUP ALL"
        ))
        .bind(("lang", target.to_string()))
        .await
        .map_err(|e| Error::Database(format!("translate live check: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("translate live check (decode): {e}")))?;
    Ok(rows.into_iter().next().map(|r| r.count > 0).unwrap_or(false))
}

// -------------------------------------------------------------------------
// DELETE /audiobook/:id
// -------------------------------------------------------------------------

#[utoipa::path(
    delete,
    path = "/audiobook/{id}",
    tag = "audiobook",
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
    assert_owner(&state, &id, &user.id).await?;
    state
        .db()
        .inner()
        .query(format!(
            "DELETE job WHERE audiobook = audiobook:`{id}`; \
             DELETE chapter WHERE audiobook = audiobook:`{id}`; \
             DELETE audiobook:`{id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("delete audiobook: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("delete audiobook: {e}")))?;

    let dir = state.config().storage_path.join(&id);
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            warn!(error = %e, path = ?dir, "delete audiobook: leaving audio dir for GC");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/generate-chapters  — kicks off async chapter writer
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/generate-chapters",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 202, description = "Accepted, generation started in background"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Already running or not in outline_ready")
    ),
    security(("bearer" = []))
)]
pub async fn generate_chapters(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    IdempotencyKey(idem): IdempotencyKey,
) -> ApiResult<StatusCode> {
    if let Some(cached) = idempotency::lookup(&state, &user.id, idem.as_deref()).await? {
        return Ok(StatusCode::from_u16(cached.status_code)
            .unwrap_or(StatusCode::ACCEPTED));
    }

    let book = load_audiobook(&state, &id).await?;
    if book.owner_id() != user.id {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        }
        .into());
    }
    let status = parse_status(&book.status)?;
    match status {
        AudiobookStatus::OutlineReady | AudiobookStatus::Failed | AudiobookStatus::TextReady => {}
        _ => {
            return Err(Error::Conflict(format!(
                "audiobook is in state {:?}; only outline_ready, text_ready, or failed can be (re)generated",
                status
            ))
            .into())
        }
    }

    if has_live_job(&state, &id, JobKind::Chapters).await? {
        return Err(Error::Conflict("chapters already in flight".into()).into());
    }

    let req = EnqueueRequest::new(JobKind::Chapters)
        .with_user(user.id.clone())
        .with_audiobook(AudiobookId(id.clone()))
        .with_max_attempts(3);
    state.jobs().enqueue(req).await?;

    idempotency::record(
        &state,
        &user.id,
        idem.as_deref(),
        "POST",
        &format!("/audiobook/{id}/generate-chapters"),
        StatusCode::ACCEPTED.as_u16(),
        "",
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/generate-audio  — TTS for every chapter
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/generate-audio",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 202, description = "Accepted, TTS started in background"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Chapters not ready to narrate")
    ),
    security(("bearer" = []))
)]
pub async fn generate_audio(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<GetOneQuery>,
    IdempotencyKey(idem): IdempotencyKey,
) -> ApiResult<StatusCode> {
    if let Some(cached) = idempotency::lookup(&state, &user.id, idem.as_deref()).await? {
        return Ok(StatusCode::from_u16(cached.status_code)
            .unwrap_or(StatusCode::ACCEPTED));
    }

    let book = load_audiobook(&state, &id).await?;
    if book.owner_id() != user.id {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        }
        .into());
    }
    let status = parse_status(&book.status)?;
    match status {
        AudiobookStatus::TextReady | AudiobookStatus::AudioReady | AudiobookStatus::Failed => {}
        _ => {
            return Err(Error::Conflict(format!(
                "audiobook is in state {:?}; chapter text must be ready before TTS",
                status
            ))
            .into())
        }
    }

    let language = q
        .language
        .unwrap_or_else(|| book.language.clone().unwrap_or_else(|| "en".to_string()));
    if !is_supported_language(&language) {
        return Err(Error::Validation(format!(
            "unsupported language `{language}`"
        ))
        .into());
    }

    if has_live_job(&state, &id, JobKind::Tts).await? {
        return Err(Error::Conflict("tts already in flight".into()).into());
    }

    let req = EnqueueRequest::new(JobKind::Tts)
        .with_user(user.id.clone())
        .with_audiobook(AudiobookId(id.clone()))
        .with_language(language)
        .with_max_attempts(3);
    state.jobs().enqueue(req).await?;

    idempotency::record(
        &state,
        &user.id,
        idem.as_deref(),
        "POST",
        &format!("/audiobook/{id}/generate-audio"),
        StatusCode::ACCEPTED.as_u16(),
        "",
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

/// Returns true iff a non-terminal job of `kind` exists for this audiobook.
/// Used to block double-submits while an identical job is still queued or
/// running.
async fn has_live_job(state: &AppState, audiobook_id: &str, kind: JobKind) -> Result<bool> {
    #[derive(Deserialize)]
    struct CountRow {
        count: i64,
    }
    let rows: Vec<CountRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM job \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND kind = $kind \
               AND status IN [\"queued\", \"running\"] \
             GROUP ALL"
        ))
        .bind(("kind", kind.as_str().to_string()))
        .await
        .map_err(|e| Error::Database(format!("live job check: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("live job check (decode): {e}")))?;
    Ok(rows.first().map(|r| r.count).unwrap_or(0) > 0)
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/chapter/:n/regenerate-audio  — single-chapter sync
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/regenerate-audio",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 200, description = "Regenerated chapter audio", body = ChapterSummary),
        (status = 404, description = "Not found"),
        (status = 502, description = "Upstream TTS error")
    ),
    security(("bearer" = []))
)]
pub async fn regenerate_chapter_audio(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Json<ChapterSummary>> {
    assert_owner(&state, &id, &user.id).await?;
    let book = load_audiobook(&state, &id).await?;
    let lang = book.language.clone().unwrap_or_else(|| "en".to_string());
    audio_gen::run_one_by_number(&state, &user.id, &id, n as i64, &lang).await?;
    let after = load_chapter_by_number(&state, &id, n as i64).await?;
    Ok(Json(after.to_summary()?))
}

// -------------------------------------------------------------------------
// PATCH /audiobook/:id/chapter/:n
// -------------------------------------------------------------------------

#[utoipa::path(
    patch,
    path = "/audiobook/{id}/chapter/{n}",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    request_body = UpdateChapterRequest,
    responses(
        (status = 200, description = "Updated chapter", body = ChapterSummary),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn patch_chapter(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
    Json(body): Json<UpdateChapterRequest>,
) -> ApiResult<Json<ChapterSummary>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    assert_owner(&state, &id, &user.id).await?;

    let chapter = load_chapter_by_number(&state, &id, n as i64).await?;
    let raw = chapter.id.id.to_raw();

    if let Some(title) = body.title {
        state
            .db()
            .inner()
            .query(format!("UPDATE chapter:`{raw}` SET title = $t"))
            .bind(("t", title.trim().to_string()))
            .await
            .map_err(|e| Error::Database(format!("patch chapter title: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch chapter title: {e}")))?;
    }
    if let Some(syn) = body.synopsis {
        state
            .db()
            .inner()
            .query(format!("UPDATE chapter:`{raw}` SET synopsis = $s"))
            .bind(("s", Some(syn.trim().to_string())))
            .await
            .map_err(|e| Error::Database(format!("patch chapter synopsis: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch chapter synopsis: {e}")))?;
    }
    if let Some(bm) = body.body_md {
        state
            .db()
            .inner()
            .query(format!(
                "UPDATE chapter:`{raw}` SET body_md = $b, status = \"text_ready\""
            ))
            .bind(("b", Some(bm)))
            .await
            .map_err(|e| Error::Database(format!("patch chapter body: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch chapter body: {e}")))?;
    }

    let after = load_chapter_by_number(&state, &id, n as i64).await?;
    Ok(Json(after.to_summary()?))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/chapter/:n/regenerate  — synchronous single-chapter
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/regenerate",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 200, description = "Regenerated chapter", body = ChapterSummary),
        (status = 404, description = "Not found"),
        (status = 502, description = "Upstream LLM error")
    ),
    security(("bearer" = []))
)]
pub async fn regenerate_chapter(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Json<ChapterSummary>> {
    assert_owner(&state, &id, &user.id).await?;
    chapter_gen::run_one_by_number(&state, &user.id, &id, n as i64).await?;
    let after = load_chapter_by_number(&state, &id, n as i64).await?;
    Ok(Json(after.to_summary()?))
}

// -------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------

async fn load_audiobook(state: &AppState, id: &str) -> Result<DbAudiobook> {
    let rows: Vec<DbAudiobook> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM audiobook:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("load audiobook: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load audiobook (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{id}"),
    })
}

async fn load_detail(
    state: &AppState,
    id: &str,
    user: &UserId,
    language_filter: Option<&str>,
) -> Result<AudiobookDetail> {
    let book = load_audiobook(state, id).await?;
    if book.owner_id() != *user {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        });
    }
    let primary = book
        .language
        .clone()
        .unwrap_or_else(|| "en".to_string());
    let active = language_filter
        .map(str::to_string)
        .unwrap_or_else(|| primary.clone());

    let all_chapters: Vec<DbChapter> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM chapter WHERE audiobook = audiobook:`{id}` \
             ORDER BY language ASC, number ASC"
        ))
        .await
        .map_err(|e| Error::Database(format!("load chapters: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load chapters (decode): {e}")))?;

    // Distinct languages, primary first.
    let mut langs: Vec<String> = all_chapters
        .iter()
        .filter_map(|c| c.language.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if !langs.contains(&primary) {
        langs.insert(0, primary.clone());
    } else {
        // Hoist primary to the front for stable UX.
        langs.retain(|l| l != &primary);
        langs.insert(0, primary.clone());
    }

    let chapters: Vec<ChapterSummary> = all_chapters
        .iter()
        .filter(|c| c.language.as_deref().unwrap_or("en") == active)
        .map(DbChapter::to_summary)
        .collect::<Result<Vec<_>>>()?;

    let mut summary = book.to_summary()?;
    summary.available_languages = langs;
    Ok(AudiobookDetail { summary, chapters })
}

/// Languages the UI exposes; mirrors the dropdown on the New Audiobook page.
/// Centralising this keeps the validation surface in one place — adding a
/// new language is a single-list edit on either side.
const SUPPORTED_LANGUAGES: &[&str] = &[
    "en", "nl", "fr", "de", "es", "it", "pt", "ru", "zh", "ja", "ko",
];

fn is_supported_language(code: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&code)
}

/// Decode a base64 cover image and write it under
/// `<storage_path>/<audiobook_id>/cover.<ext>`, then update the audiobook's
/// `cover_path` column. The on-disk extension matches the sniffed MIME so
/// the stream handler can serve it with the right Content-Type.
async fn persist_cover(state: &AppState, audiobook_id: &str, b64: &str) -> Result<()> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

    let trimmed = b64.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    // Tolerate `data:image/png;base64,...` payloads from less-careful clients.
    let raw = trimmed
        .find(";base64,")
        .map(|i| &trimmed[(i + ";base64,".len())..])
        .unwrap_or(trimmed);
    let bytes = B64
        .decode(raw.as_bytes())
        .map_err(|e| Error::Validation(format!("cover_image_base64: {e}")))?;
    if bytes.is_empty() {
        return Err(Error::Validation("cover_image_base64 is empty".into()));
    }
    let ext = match crate::handlers::cover::detect_mime(&bytes) {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "bin",
    };
    let dir = state.config().storage_path.join(audiobook_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Other(anyhow::anyhow!("create cover dir {dir:?}: {e}")))?;
    let filename = format!("cover.{ext}");
    let path = dir.join(&filename);
    std::fs::write(&path, &bytes)
        .map_err(|e| Error::Other(anyhow::anyhow!("write cover {path:?}: {e}")))?;

    let rel = format!("{audiobook_id}/{filename}");
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook:`{audiobook_id}` SET cover_path = $p"
        ))
        .bind(("p", rel))
        .await
        .map_err(|e| Error::Database(format!("set cover_path: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set cover_path: {e}")))?;
    Ok(())
}

async fn assert_voice_enabled(state: &AppState, voice_id: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Row {
        id: Thing,
        enabled: bool,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, enabled FROM voice:`{voice_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("voice lookup: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("voice lookup (decode): {e}")))?;
    let row = rows
        .into_iter()
        .next()
        .ok_or_else(|| Error::Validation(format!("unknown voice `{voice_id}`")))?;
    if !row.enabled {
        return Err(Error::Validation(format!("voice `{voice_id}` is disabled")));
    }
    Ok(row.id.id.to_raw())
}

async fn assert_owner(state: &AppState, id: &str, user: &UserId) -> Result<()> {
    let book = load_audiobook(state, id).await?;
    if book.owner_id() != *user {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        });
    }
    Ok(())
}

async fn load_chapter_by_number(state: &AppState, audiobook_id: &str, n: i64) -> Result<DbChapter> {
    // Default to the audiobook's primary language. Editing endpoints
    // (patch/regenerate) currently target the primary version; translations
    // get their own update path.
    let book = load_audiobook(state, audiobook_id).await?;
    let lang = book.language.clone().unwrap_or_else(|| "en".to_string());
    let rows: Vec<DbChapter> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM chapter WHERE audiobook = audiobook:`{audiobook_id}` \
             AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", n))
        .bind(("lang", lang))
        .await
        .map_err(|e| Error::Database(format!("load chapter: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load chapter (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{audiobook_id} chapter {n}"),
    })
}
