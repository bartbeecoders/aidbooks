//! Audiobook CRUD + content-generation triggers.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use listenai_core::domain::{AudiobookLength, AudiobookStatus, ChapterStatus};
use listenai_core::id::{AudiobookId, ChapterId, UserId};
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use tracing::{error, info};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::generation::{audio as audio_gen, chapter as chapter_gen, outline as outline_gen};
use crate::state::AppState;

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
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookSummary {
    pub id: AudiobookId,
    pub title: String,
    pub topic: String,
    pub genre: Option<String>,
    pub length: AudiobookLength,
    pub status: AudiobookStatus,
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
    let sql = format!(
        r#"CREATE audiobook:`{id}` CONTENT {{
            owner: user:`{user_id}`,
            title: "Untitled",
            topic: $topic,
            genre: $genre,
            length: $length,
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
        .await
        .map_err(|e| Error::Database(format!("create audiobook: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create audiobook: {e}")))?;

    outline_gen::run(
        &state,
        &user.id,
        &id,
        &body.topic,
        body.length,
        if genre.is_empty() { "any" } else { &genre },
    )
    .await?;

    Ok(Json(load_detail(&state, &id, &user.id).await?))
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

#[utoipa::path(
    get,
    path = "/audiobook/{id}",
    tag = "audiobook",
    params(("id" = String, Path, description = "Audiobook id")),
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
) -> ApiResult<Json<AudiobookDetail>> {
    Ok(Json(load_detail(&state, &id, &user.id).await?))
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

    Ok(Json(load_detail(&state, &id, &user.id).await?))
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
            "DELETE chapter WHERE audiobook = audiobook:`{id}`; \
             DELETE audiobook:`{id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("delete audiobook: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("delete audiobook: {e}")))?;
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
) -> ApiResult<StatusCode> {
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

    // Async, fire-and-forget. Phase 5 will promote this to a durable job.
    let state_clone = state.clone();
    let user_id = user.id.clone();
    let audiobook_id = id.clone();
    tokio::spawn(async move {
        if let Err(e) = chapter_gen::run_all(&state_clone, &user_id, &audiobook_id).await {
            error!(
                audiobook = audiobook_id,
                error = %e,
                "background chapter generation failed"
            );
        } else {
            info!(
                audiobook = audiobook_id,
                "background chapter generation complete"
            );
        }
    });

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
) -> ApiResult<StatusCode> {
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

    let state_clone = state.clone();
    let user_id = user.id.clone();
    let audiobook_id = id.clone();
    tokio::spawn(async move {
        if let Err(e) = audio_gen::run_all(&state_clone, &user_id, &audiobook_id).await {
            error!(
                audiobook = audiobook_id,
                error = %e,
                "background audio generation failed"
            );
        } else {
            info!(
                audiobook = audiobook_id,
                "background audio generation complete"
            );
        }
    });

    Ok(StatusCode::ACCEPTED)
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
    audio_gen::run_one_by_number(&state, &user.id, &id, n as i64).await?;
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

async fn load_detail(state: &AppState, id: &str, user: &UserId) -> Result<AudiobookDetail> {
    let book = load_audiobook(state, id).await?;
    if book.owner_id() != *user {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        });
    }
    let chapters: Vec<DbChapter> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM chapter WHERE audiobook = audiobook:`{id}` ORDER BY number ASC"
        ))
        .await
        .map_err(|e| Error::Database(format!("load chapters: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load chapters (decode): {e}")))?;
    let summaries = chapters
        .iter()
        .map(DbChapter::to_summary)
        .collect::<Result<Vec<_>>>()?;
    Ok(AudiobookDetail {
        summary: book.to_summary()?,
        chapters: summaries,
    })
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
    let rows: Vec<DbChapter> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM chapter WHERE audiobook = audiobook:`{audiobook_id}` AND number = $n LIMIT 1"
        ))
        .bind(("n", n))
        .await
        .map_err(|e| Error::Database(format!("load chapter: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load chapter (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{audiobook_id} chapter {n}"),
    })
}
