//! Topic templates: admin-curated starting points the New Audiobook page
//! offers as a dropdown. Authenticated users get the enabled subset; admins
//! get full CRUD.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use listenai_core::domain::AudiobookLength;
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::{Authenticated, RequireAdmin};
use crate::error::ApiResult;
use crate::state::AppState;

// -------------------------------------------------------------------------
// DTOs
// -------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct TopicTemplate {
    pub id: String,
    pub title: String,
    pub topic: String,
    pub genre: Option<String>,
    pub length: Option<AudiobookLength>,
    pub language: Option<String>,
    pub sort_order: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TopicTemplateList {
    pub items: Vec<TopicTemplate>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateTopicTemplateRequest {
    #[validate(length(min = 1, max = 120))]
    pub title: String,
    #[validate(length(min = 1, max = 1000))]
    pub topic: String,
    #[validate(length(max = 40))]
    pub genre: Option<String>,
    pub length: Option<AudiobookLength>,
    #[validate(length(min = 2, max = 8))]
    pub language: Option<String>,
    pub sort_order: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateTopicTemplateRequest {
    #[validate(length(min = 1, max = 120))]
    pub title: Option<String>,
    #[validate(length(min = 1, max = 1000))]
    pub topic: Option<String>,
    /// Pass an empty string to clear the genre.
    #[validate(length(max = 40))]
    pub genre: Option<String>,
    /// `null` clears the length default.
    pub length: Option<Option<AudiobookLength>>,
    /// Pass an empty string to clear the language default.
    #[validate(length(max = 8))]
    pub language: Option<String>,
    pub sort_order: Option<i32>,
    pub enabled: Option<bool>,
}

// -------------------------------------------------------------------------
// Internal row type
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DbRow {
    id: Thing,
    title: String,
    topic: String,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    length: Option<String>,
    #[serde(default)]
    language: Option<String>,
    sort_order: i64,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl DbRow {
    fn into_dto(self) -> Result<TopicTemplate> {
        Ok(TopicTemplate {
            id: self.id.id.to_raw(),
            title: self.title,
            topic: self.topic,
            genre: self.genre,
            length: match self.length.as_deref() {
                None => None,
                Some("short") => Some(AudiobookLength::Short),
                Some("medium") => Some(AudiobookLength::Medium),
                Some("long") => Some(AudiobookLength::Long),
                Some(other) => {
                    return Err(Error::Database(format!(
                        "topic_template.length unknown `{other}`"
                    )))
                }
            },
            language: self.language,
            sort_order: self.sort_order as i32,
            enabled: self.enabled,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

fn length_to_str(l: AudiobookLength) -> &'static str {
    match l {
        AudiobookLength::Short => "short",
        AudiobookLength::Medium => "medium",
        AudiobookLength::Long => "long",
    }
}

// -------------------------------------------------------------------------
// Public: GET /topic-templates  (auth required, enabled only)
// -------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/topic-templates",
    tag = "topics",
    responses(
        (status = 200, description = "Enabled topic templates", body = TopicTemplateList),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list_public(
    State(state): State<AppState>,
    Authenticated(_user): Authenticated,
) -> ApiResult<Json<TopicTemplateList>> {
    let rows: Vec<DbRow> = state
        .db()
        .inner()
        .query(
            "SELECT * FROM topic_template WHERE enabled = true \
             ORDER BY sort_order ASC, title ASC",
        )
        .await
        .map_err(|e| Error::Database(format!("topic_template list: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("topic_template list (decode): {e}")))?;
    let items = rows
        .into_iter()
        .map(DbRow::into_dto)
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(TopicTemplateList { items }))
}

// -------------------------------------------------------------------------
// Admin: GET /admin/topic-templates
// -------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/admin/topic-templates",
    tag = "admin",
    responses(
        (status = 200, description = "All topic templates", body = TopicTemplateList),
        (status = 403, description = "Not an admin")
    ),
    security(("bearer" = []))
)]
pub async fn list_admin(
    State(state): State<AppState>,
    _admin: RequireAdmin,
) -> ApiResult<Json<TopicTemplateList>> {
    let rows: Vec<DbRow> = state
        .db()
        .inner()
        .query(
            "SELECT * FROM topic_template ORDER BY sort_order ASC, title ASC",
        )
        .await
        .map_err(|e| Error::Database(format!("admin topic_template list: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("admin topic_template list (decode): {e}")))?;
    let items = rows
        .into_iter()
        .map(DbRow::into_dto)
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(TopicTemplateList { items }))
}

// -------------------------------------------------------------------------
// Admin: POST /admin/topic-templates
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/admin/topic-templates",
    tag = "admin",
    request_body = CreateTopicTemplateRequest,
    responses(
        (status = 201, description = "Created template", body = TopicTemplate),
        (status = 400, description = "Validation failed"),
        (status = 403, description = "Not an admin")
    ),
    security(("bearer" = []))
)]
pub async fn create(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Json(body): Json<CreateTopicTemplateRequest>,
) -> ApiResult<(StatusCode, Json<TopicTemplate>)> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    let id = Uuid::new_v4().simple().to_string();
    let length_str = body.length.map(length_to_str).map(str::to_string);
    let genre = body
        .genre
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let language = body
        .language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let sort_order = body.sort_order.unwrap_or(0) as i64;
    let enabled = body.enabled.unwrap_or(true);

    state
        .db()
        .inner()
        .query(format!(
            r#"CREATE topic_template:`{id}` CONTENT {{
                title: $title,
                topic: $topic,
                genre: $genre,
                length: $length,
                language: $language,
                sort_order: $sort_order,
                enabled: $enabled
            }}"#
        ))
        .bind(("title", body.title.trim().to_string()))
        .bind(("topic", body.topic.trim().to_string()))
        .bind(("genre", genre))
        .bind(("length", length_str))
        .bind(("language", language))
        .bind(("sort_order", sort_order))
        .bind(("enabled", enabled))
        .await
        .map_err(|e| Error::Database(format!("topic_template create: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("topic_template create: {e}")))?;

    Ok((StatusCode::CREATED, Json(load(&state, &id).await?)))
}

// -------------------------------------------------------------------------
// Admin: PATCH /admin/topic-templates/:id
// -------------------------------------------------------------------------

#[utoipa::path(
    patch,
    path = "/admin/topic-templates/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = UpdateTopicTemplateRequest,
    responses(
        (status = 200, description = "Updated template", body = TopicTemplate),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found"),
        (status = 403, description = "Not an admin")
    ),
    security(("bearer" = []))
)]
pub async fn patch(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateTopicTemplateRequest>,
) -> ApiResult<Json<TopicTemplate>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    if !is_valid_id(&id) {
        return Err(Error::Validation("invalid id".into()).into());
    }

    let mut sets: Vec<&str> = Vec::new();
    if body.title.is_some() {
        sets.push("title = $title");
    }
    if body.topic.is_some() {
        sets.push("topic = $topic");
    }
    if body.genre.is_some() {
        sets.push("genre = $genre");
    }
    if body.length.is_some() {
        sets.push("length = $length");
    }
    if body.language.is_some() {
        sets.push("language = $language");
    }
    if body.sort_order.is_some() {
        sets.push("sort_order = $sort_order");
    }
    if body.enabled.is_some() {
        sets.push("enabled = $enabled");
    }
    if sets.is_empty() {
        return Err(Error::Validation("no fields to update".into()).into());
    }

    // Empty-string collapses to NONE so the column actually clears rather
    // than carrying a zero-length default forever.
    let genre_clear = body.genre.as_ref().map(|g| {
        let t = g.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });
    let language_clear = body.language.as_ref().map(|l| {
        let t = l.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });
    let length_clear = body
        .length
        .as_ref()
        .map(|opt| opt.map(length_to_str).map(str::to_string));

    let sql = format!("UPDATE topic_template:`{id}` SET {}", sets.join(", "));
    state
        .db()
        .inner()
        .query(sql)
        .bind(("title", body.title.map(|s| s.trim().to_string())))
        .bind(("topic", body.topic.map(|s| s.trim().to_string())))
        .bind(("genre", genre_clear))
        .bind(("length", length_clear))
        .bind(("language", language_clear))
        .bind(("sort_order", body.sort_order.map(|n| n as i64)))
        .bind(("enabled", body.enabled))
        .await
        .map_err(|e| Error::Database(format!("topic_template patch: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("topic_template patch: {e}")))?;

    Ok(Json(load(&state, &id).await?))
}

// -------------------------------------------------------------------------
// Admin: DELETE /admin/topic-templates/:id
// -------------------------------------------------------------------------

#[utoipa::path(
    delete,
    path = "/admin/topic-templates/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found"),
        (status = 403, description = "Not an admin")
    ),
    security(("bearer" = []))
)]
pub async fn delete(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if !is_valid_id(&id) {
        return Err(Error::Validation("invalid id".into()).into());
    }
    state
        .db()
        .inner()
        .query(format!("DELETE topic_template:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("topic_template delete: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("topic_template delete: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

// -------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------

async fn load(state: &AppState, id: &str) -> Result<TopicTemplate> {
    let rows: Vec<DbRow> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM topic_template:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("topic_template load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("topic_template load (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("topic_template:{id}"),
    })?
        .into_dto()
}

/// Whitelist `[a-z0-9]+` ids to keep the embedded `topic_template:`<id>``
/// safe from injection — we always create with a uuid simple, so this is
/// just defence in depth on the PATCH/DELETE paths.
fn is_valid_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}
