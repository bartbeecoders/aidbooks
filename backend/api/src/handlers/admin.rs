//! Admin-only endpoints — runtime-editable LLMs, voices, users, and jobs.
//!
//! Everything here is gated by [`crate::auth::RequireAdmin`]. The extractor
//! returns 403 for non-admins and 401 for unauthenticated requests, so no
//! route here needs to re-check the role.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use listenai_core::domain::{JobKind, JobStatus, UserRole, UserTier};
use listenai_core::id::{JobId, LlmId, UserId, VoiceId};
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use utoipa::ToSchema;
use validator::Validate;

use crate::auth::RequireAdmin;
use crate::error::ApiResult;
use crate::llm::{ChatMessage, ChatRequest};
use crate::state::AppState;

// =========================================================================
// LLM admin
// =========================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminLlmRow {
    pub id: LlmId,
    pub name: String,
    pub provider: String,
    pub model_id: String,
    pub context_window: u32,
    pub cost_prompt_per_1k: f64,
    pub cost_completion_per_1k: f64,
    pub enabled: bool,
    pub default_for: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminLlmList {
    pub items: Vec<AdminLlmRow>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateLlmRequest {
    pub enabled: Option<bool>,
    #[validate(length(min = 1, max = 80))]
    pub name: Option<String>,
    #[validate(range(min = 0.0, max = 1000.0))]
    pub cost_prompt_per_1k: Option<f64>,
    #[validate(range(min = 0.0, max = 1000.0))]
    pub cost_completion_per_1k: Option<f64>,
    pub default_for: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateLlmRequest {
    /// Snake-case identifier used as the SurrealDB record id (e.g.
    /// `gemini_flash_image`). Restricted to `[a-z0-9_]` so it can be
    /// embedded safely in `llm:`<id>``; validated separately in the handler.
    #[validate(length(min = 1, max = 64))]
    pub id: String,
    #[validate(length(min = 1, max = 80))]
    pub name: String,
    /// Upstream model id, e.g. `google/gemini-2.5-flash-image`.
    #[validate(length(min = 1, max = 200))]
    pub model_id: String,
    #[validate(range(min = 1, max = 10_000_000))]
    pub context_window: u32,
    #[validate(range(min = 0.0, max = 1000.0))]
    pub cost_prompt_per_1k: f64,
    #[validate(range(min = 0.0, max = 1000.0))]
    pub cost_completion_per_1k: f64,
    pub enabled: Option<bool>,
    pub default_for: Option<Vec<String>>,
}

fn is_valid_llm_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[derive(Debug, Deserialize)]
struct DbLlm {
    id: Thing,
    name: String,
    provider: String,
    model_id: String,
    context_window: i64,
    cost_prompt_per_1k: f64,
    cost_completion_per_1k: f64,
    enabled: bool,
    default_for: Vec<String>,
}

#[utoipa::path(
    get, path = "/admin/llm", tag = "admin",
    responses(
        (status = 200, body = AdminLlmList),
        (status = 401), (status = 403),
    ),
    security(("bearer" = []))
)]
pub async fn list_llms(
    State(state): State<AppState>,
    _admin: RequireAdmin,
) -> ApiResult<Json<AdminLlmList>> {
    let rows: Vec<DbLlm> = state
        .db()
        .inner()
        .query("SELECT * FROM llm ORDER BY name ASC")
        .await
        .map_err(|e| Error::Database(format!("admin list_llms: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("admin list_llms (decode): {e}")))?;
    let items = rows.into_iter().map(row_to_llm).collect();
    Ok(Json(AdminLlmList { items }))
}

#[utoipa::path(
    patch, path = "/admin/llm/{id}", tag = "admin",
    params(("id" = String, Path)),
    request_body = UpdateLlmRequest,
    responses((status = 200, body = AdminLlmRow), (status = 404), (status = 403)),
    security(("bearer" = []))
)]
pub async fn patch_llm(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateLlmRequest>,
) -> ApiResult<Json<AdminLlmRow>> {
    body.validate().map_err(|e| Error::Validation(e.to_string()))?;
    let mut sets: Vec<String> = Vec::new();
    if body.enabled.is_some() {
        sets.push("enabled = $enabled".into());
    }
    if body.name.is_some() {
        sets.push("name = $name".into());
    }
    if body.cost_prompt_per_1k.is_some() {
        sets.push("cost_prompt_per_1k = $cp".into());
    }
    if body.cost_completion_per_1k.is_some() {
        sets.push("cost_completion_per_1k = $cc".into());
    }
    if body.default_for.is_some() {
        sets.push("default_for = $df".into());
    }
    if sets.is_empty() {
        return Err(Error::Validation("no fields to update".into()).into());
    }

    let sql = format!("UPDATE llm:`{id}` SET {}", sets.join(", "));
    state
        .db()
        .inner()
        .query(sql)
        .bind(("enabled", body.enabled))
        .bind(("name", body.name))
        .bind(("cp", body.cost_prompt_per_1k))
        .bind(("cc", body.cost_completion_per_1k))
        .bind(("df", body.default_for))
        .await
        .map_err(|e| Error::Database(format!("admin patch_llm: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("admin patch_llm: {e}")))?;

    Ok(Json(load_llm(&state, &id).await?))
}

#[utoipa::path(
    post, path = "/admin/llm", tag = "admin",
    request_body = CreateLlmRequest,
    responses(
        (status = 201, body = AdminLlmRow),
        (status = 400, description = "Validation failed"),
        (status = 409, description = "An LLM with this id already exists"),
        (status = 403),
    ),
    security(("bearer" = []))
)]
pub async fn create_llm(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Json(body): Json<CreateLlmRequest>,
) -> ApiResult<(StatusCode, Json<AdminLlmRow>)> {
    body.validate().map_err(|e| Error::Validation(e.to_string()))?;
    if !is_valid_llm_id(&body.id) {
        return Err(Error::Validation(
            "id must be lowercase letters, digits, or underscores".into(),
        )
        .into());
    }

    // 409 on collision so the admin gets a clean error rather than silently
    // overwriting an existing row.
    #[derive(Deserialize)]
    struct ExistsRow {
        #[serde(rename = "id")]
        _id: Thing,
    }
    let existing: Vec<ExistsRow> = state
        .db()
        .inner()
        .query(format!("SELECT id FROM llm:`{}`", body.id))
        .await
        .map_err(|e| Error::Database(format!("create_llm exists: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("create_llm exists (decode): {e}")))?;
    if !existing.is_empty() {
        return Err(Error::Conflict(format!("llm `{}` already exists", body.id)).into());
    }

    let enabled = body.enabled.unwrap_or(true);
    let default_for = body.default_for.unwrap_or_default();

    state
        .db()
        .inner()
        .query(format!(
            r#"CREATE llm:`{}` CONTENT {{
                name: $name,
                provider: "open_router",
                model_id: $model_id,
                context_window: $cw,
                cost_prompt_per_1k: $cp,
                cost_completion_per_1k: $cc,
                enabled: $enabled,
                default_for: $df
            }}"#,
            body.id
        ))
        .bind(("name", body.name))
        .bind(("model_id", body.model_id))
        .bind(("cw", body.context_window as i64))
        .bind(("cp", body.cost_prompt_per_1k))
        .bind(("cc", body.cost_completion_per_1k))
        .bind(("enabled", enabled))
        .bind(("df", default_for))
        .await
        .map_err(|e| Error::Database(format!("create_llm: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create_llm: {e}")))?;

    Ok((StatusCode::CREATED, Json(load_llm(&state, &body.id).await?)))
}

async fn load_llm(state: &AppState, id: &str) -> Result<AdminLlmRow> {
    let rows: Vec<DbLlm> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM llm:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("load_llm: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load_llm (decode): {e}")))?;
    rows.into_iter().next().map(row_to_llm).ok_or(Error::NotFound {
        resource: format!("llm:{id}"),
    })
}

fn row_to_llm(r: DbLlm) -> AdminLlmRow {
    AdminLlmRow {
        id: LlmId(r.id.id.to_raw()),
        name: r.name,
        provider: r.provider,
        model_id: r.model_id,
        context_window: r.context_window as u32,
        cost_prompt_per_1k: r.cost_prompt_per_1k,
        cost_completion_per_1k: r.cost_completion_per_1k,
        enabled: r.enabled,
        default_for: r.default_for,
    }
}

// =========================================================================
// Voice admin
// =========================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminVoiceRow {
    pub id: VoiceId,
    pub name: String,
    pub provider: String,
    pub provider_voice_id: String,
    pub gender: String,
    pub accent: String,
    pub language: String,
    pub sample_url: Option<String>,
    pub enabled: bool,
    pub premium_only: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminVoiceList {
    pub items: Vec<AdminVoiceRow>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateVoiceRequest {
    pub enabled: Option<bool>,
    pub premium_only: Option<bool>,
    #[validate(length(min = 1, max = 80))]
    pub name: Option<String>,
    #[validate(length(max = 40))]
    pub accent: Option<String>,
    #[validate(length(max = 500))]
    pub sample_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DbVoice {
    id: Thing,
    name: String,
    provider: String,
    provider_voice_id: String,
    gender: String,
    accent: String,
    language: String,
    sample_url: Option<String>,
    enabled: bool,
    premium_only: bool,
}

#[utoipa::path(
    get, path = "/admin/voice", tag = "admin",
    responses((status = 200, body = AdminVoiceList), (status = 403)),
    security(("bearer" = []))
)]
pub async fn list_voices(
    State(state): State<AppState>,
    _admin: RequireAdmin,
) -> ApiResult<Json<AdminVoiceList>> {
    let rows: Vec<DbVoice> = state
        .db()
        .inner()
        .query("SELECT * FROM voice ORDER BY name ASC")
        .await
        .map_err(|e| Error::Database(format!("admin list_voices: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("admin list_voices (decode): {e}")))?;
    let items = rows.into_iter().map(row_to_voice).collect();
    Ok(Json(AdminVoiceList { items }))
}

#[utoipa::path(
    patch, path = "/admin/voice/{id}", tag = "admin",
    params(("id" = String, Path)),
    request_body = UpdateVoiceRequest,
    responses((status = 200, body = AdminVoiceRow), (status = 404), (status = 403)),
    security(("bearer" = []))
)]
pub async fn patch_voice(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateVoiceRequest>,
) -> ApiResult<Json<AdminVoiceRow>> {
    body.validate().map_err(|e| Error::Validation(e.to_string()))?;
    let mut sets: Vec<String> = Vec::new();
    if body.enabled.is_some() {
        sets.push("enabled = $enabled".into());
    }
    if body.premium_only.is_some() {
        sets.push("premium_only = $premium".into());
    }
    if body.name.is_some() {
        sets.push("name = $name".into());
    }
    if body.accent.is_some() {
        sets.push("accent = $accent".into());
    }
    if body.sample_url.is_some() {
        sets.push("sample_url = $sample_url".into());
    }
    if sets.is_empty() {
        return Err(Error::Validation("no fields to update".into()).into());
    }

    let sql = format!("UPDATE voice:`{id}` SET {}", sets.join(", "));
    state
        .db()
        .inner()
        .query(sql)
        .bind(("enabled", body.enabled))
        .bind(("premium", body.premium_only))
        .bind(("name", body.name))
        .bind(("accent", body.accent))
        .bind(("sample_url", body.sample_url))
        .await
        .map_err(|e| Error::Database(format!("admin patch_voice: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("admin patch_voice: {e}")))?;

    Ok(Json(load_voice(&state, &id).await?))
}

async fn load_voice(state: &AppState, id: &str) -> Result<AdminVoiceRow> {
    let rows: Vec<DbVoice> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM voice:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("load_voice: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load_voice (decode): {e}")))?;
    rows.into_iter().next().map(row_to_voice).ok_or(Error::NotFound {
        resource: format!("voice:{id}"),
    })
}

fn row_to_voice(r: DbVoice) -> AdminVoiceRow {
    AdminVoiceRow {
        id: VoiceId(r.id.id.to_raw()),
        name: r.name,
        provider: r.provider,
        provider_voice_id: r.provider_voice_id,
        gender: r.gender,
        accent: r.accent,
        language: r.language,
        sample_url: r.sample_url,
        enabled: r.enabled,
        premium_only: r.premium_only,
    }
}

// =========================================================================
// User admin
// =========================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminUserRow {
    pub id: UserId,
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
    pub tier: UserTier,
    pub created_at: DateTime<Utc>,
    pub email_verified_at: Option<DateTime<Utc>>,
    /// Count of non-revoked, non-expired sessions at query time.
    pub active_sessions: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminUserList {
    pub items: Vec<AdminUserRow>,
    pub total: u32,
}

#[derive(Debug, Deserialize)]
pub struct ListUsersQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub role: Option<UserRole>,
    #[serde(default)]
    pub tier: Option<UserTier>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateUserRequest {
    pub role: Option<UserRole>,
    pub tier: Option<UserTier>,
}

#[derive(Debug, Deserialize)]
struct DbUser {
    id: Thing,
    email: String,
    display_name: String,
    role: String,
    tier: String,
    created_at: DateTime<Utc>,
    email_verified_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    get, path = "/admin/users", tag = "admin",
    responses((status = 200, body = AdminUserList), (status = 403)),
    security(("bearer" = []))
)]
pub async fn list_users(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<ListUsersQuery>,
) -> ApiResult<Json<AdminUserList>> {
    let limit = q.limit.unwrap_or(100).min(500);
    let search = q.q.unwrap_or_default();
    let role_filter = q.role.map(|r| match r {
        UserRole::Admin => "admin",
        UserRole::User => "user",
    });
    let tier_filter = q.tier.map(|t| match t {
        UserTier::Free => "free",
        UserTier::Pro => "pro",
    });

    let mut where_parts: Vec<&str> = Vec::new();
    if !search.is_empty() {
        where_parts.push("string::contains(string::lowercase(email), string::lowercase($q))");
    }
    if role_filter.is_some() {
        where_parts.push("role = $role");
    }
    if tier_filter.is_some() {
        where_parts.push("tier = $tier");
    }
    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };
    let sql = format!(
        "SELECT * FROM user {where_clause} ORDER BY created_at DESC LIMIT {limit}",
    );

    let rows: Vec<DbUser> = state
        .db()
        .inner()
        .query(sql)
        .bind(("q", search))
        .bind(("role", role_filter.map(str::to_string)))
        .bind(("tier", tier_filter.map(str::to_string)))
        .await
        .map_err(|e| Error::Database(format!("admin list_users: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("admin list_users (decode): {e}")))?;

    // Parallel session-count for the listed users. `len(rows)` is bounded by
    // `limit` (≤ 500) so N round-trips is acceptable. A single aggregate
    // query is possible but noticeably more SurrealQL-fiddly.
    let mut items: Vec<AdminUserRow> = Vec::with_capacity(rows.len());
    for r in rows {
        let raw = r.id.id.to_raw();
        let sessions = count_active_sessions(&state, &raw).await.unwrap_or(0);
        items.push(AdminUserRow {
            id: UserId(raw),
            email: r.email,
            display_name: r.display_name,
            role: parse_role(&r.role)?,
            tier: parse_tier(&r.tier)?,
            created_at: r.created_at,
            email_verified_at: r.email_verified_at,
            active_sessions: sessions,
        });
    }
    let total = items.len() as u32;
    Ok(Json(AdminUserList { items, total }))
}

#[utoipa::path(
    patch, path = "/admin/users/{id}", tag = "admin",
    params(("id" = String, Path)),
    request_body = UpdateUserRequest,
    responses((status = 200, body = AdminUserRow), (status = 404), (status = 403)),
    security(("bearer" = []))
)]
pub async fn patch_user(
    State(state): State<AppState>,
    admin: RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> ApiResult<Json<AdminUserRow>> {
    let admin = admin.0;
    // Guard: an admin cannot demote themselves — prevents locking everyone
    // out. They still can with direct DB access; this just blocks the UI.
    if admin.id.0 == id && matches!(body.role, Some(UserRole::User)) {
        return Err(Error::Conflict(
            "you cannot demote your own admin account from the admin UI".into(),
        )
        .into());
    }
    let mut sets: Vec<String> = Vec::new();
    if body.role.is_some() {
        sets.push("role = $role".into());
    }
    if body.tier.is_some() {
        sets.push("tier = $tier".into());
    }
    if sets.is_empty() {
        return Err(Error::Validation("no fields to update".into()).into());
    }
    let sql = format!("UPDATE user:`{id}` SET {}", sets.join(", "));
    let role_s = body.role.map(|r| match r {
        UserRole::Admin => "admin",
        UserRole::User => "user",
    });
    let tier_s = body.tier.map(|t| match t {
        UserTier::Free => "free",
        UserTier::Pro => "pro",
    });
    state
        .db()
        .inner()
        .query(sql)
        .bind(("role", role_s.map(str::to_string)))
        .bind(("tier", tier_s.map(str::to_string)))
        .await
        .map_err(|e| Error::Database(format!("admin patch_user: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("admin patch_user: {e}")))?;

    Ok(Json(load_user_row(&state, &id).await?))
}

#[utoipa::path(
    post, path = "/admin/users/{id}/revoke-sessions", tag = "admin",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "Count of sessions revoked", body = RevokeSessionsResponse),
        (status = 403), (status = 404),
    ),
    security(("bearer" = []))
)]
pub async fn revoke_sessions(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
) -> ApiResult<Json<RevokeSessionsResponse>> {
    let rows: Vec<Thing> = state
        .db()
        .inner()
        .query(format!(
            "SELECT VALUE id FROM session \
             WHERE user = user:`{id}` AND revoked_at = NONE"
        ))
        .await
        .map_err(|e| Error::Database(format!("admin revoke list: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("admin revoke list (decode): {e}")))?;
    let n = rows.len() as u32;
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE session SET revoked_at = time::now() \
             WHERE user = user:`{id}` AND revoked_at = NONE"
        ))
        .await
        .map_err(|e| Error::Database(format!("admin revoke: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("admin revoke: {e}")))?;
    Ok(Json(RevokeSessionsResponse { revoked: n }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RevokeSessionsResponse {
    pub revoked: u32,
}

async fn count_active_sessions(state: &AppState, user_raw: &str) -> Result<u32> {
    #[derive(Deserialize)]
    struct Row {
        count: i64,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM session \
             WHERE user = user:`{user_raw}` AND revoked_at = NONE \
               AND expires_at > time::now() \
             GROUP ALL"
        ))
        .await
        .map_err(|e| Error::Database(format!("count sessions: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("count sessions (decode): {e}")))?;
    Ok(rows.first().map(|r| r.count as u32).unwrap_or(0))
}

async fn load_user_row(state: &AppState, id: &str) -> Result<AdminUserRow> {
    let rows: Vec<DbUser> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM user:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("load_user: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load_user (decode): {e}")))?;
    let r = rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("user:{id}"),
    })?;
    let raw = r.id.id.to_raw();
    let sessions = count_active_sessions(state, &raw).await.unwrap_or(0);
    Ok(AdminUserRow {
        id: UserId(raw),
        email: r.email,
        display_name: r.display_name,
        role: parse_role(&r.role)?,
        tier: parse_tier(&r.tier)?,
        created_at: r.created_at,
        email_verified_at: r.email_verified_at,
        active_sessions: sessions,
    })
}

fn parse_role(s: &str) -> Result<UserRole> {
    Ok(match s {
        "admin" => UserRole::Admin,
        "user" => UserRole::User,
        other => return Err(Error::Database(format!("unknown role `{other}`"))),
    })
}

fn parse_tier(s: &str) -> Result<UserTier> {
    Ok(match s {
        "free" => UserTier::Free,
        "pro" => UserTier::Pro,
        other => return Err(Error::Database(format!("unknown tier `{other}`"))),
    })
}

// =========================================================================
// Jobs admin
// =========================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminJobRow {
    pub id: JobId,
    pub kind: JobKind,
    pub status: JobStatus,
    pub audiobook_id: Option<String>,
    pub user_id: Option<String>,
    pub parent_id: Option<String>,
    pub chapter_number: Option<u32>,
    pub progress_pct: f32,
    pub attempts: u32,
    pub max_attempts: u32,
    pub last_error: Option<String>,
    pub queued_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminJobList {
    pub items: Vec<AdminJobRow>,
}

#[derive(Debug, Deserialize)]
pub struct ListJobsQuery {
    #[serde(default)]
    pub status: Option<String>, // accepts comma-separated list
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct DbJob {
    id: Thing,
    kind: String,
    audiobook: Option<Thing>,
    user: Option<Thing>,
    parent: Option<Thing>,
    chapter_number: Option<i64>,
    status: String,
    progress_pct: f32,
    attempts: i64,
    max_attempts: i64,
    last_error: Option<String>,
    queued_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    get, path = "/admin/jobs", tag = "admin",
    responses((status = 200, body = AdminJobList), (status = 403)),
    security(("bearer" = []))
)]
pub async fn list_jobs(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<ListJobsQuery>,
) -> ApiResult<Json<AdminJobList>> {
    let limit = q.limit.unwrap_or(100).min(500);
    let statuses: Vec<String> = q
        .status
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_string())
        .collect();
    let kinds: Vec<String> = q
        .kind
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_string())
        .collect();

    let mut where_parts: Vec<&str> = Vec::new();
    if !statuses.is_empty() {
        where_parts.push("status INSIDE $statuses");
    }
    if !kinds.is_empty() {
        where_parts.push("kind INSIDE $kinds");
    }
    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };
    let sql = format!(
        "SELECT * FROM job {where_clause} ORDER BY queued_at DESC LIMIT {limit}"
    );
    let rows: Vec<DbJob> = state
        .db()
        .inner()
        .query(sql)
        .bind(("statuses", statuses))
        .bind(("kinds", kinds))
        .await
        .map_err(|e| Error::Database(format!("admin list_jobs: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("admin list_jobs (decode): {e}")))?;

    let items = rows
        .into_iter()
        .map(|r| {
            Ok::<AdminJobRow, Error>(AdminJobRow {
                id: JobId(r.id.id.to_raw()),
                kind: JobKind::parse(&r.kind)
                    .ok_or_else(|| Error::Database(format!("unknown kind `{}`", r.kind)))?,
                status: JobStatus::parse(&r.status)
                    .ok_or_else(|| Error::Database(format!("unknown status `{}`", r.status)))?,
                audiobook_id: r.audiobook.map(|t| t.id.to_raw()),
                user_id: r.user.map(|t| t.id.to_raw()),
                parent_id: r.parent.map(|t| t.id.to_raw()),
                chapter_number: r.chapter_number.map(|c| c as u32),
                progress_pct: r.progress_pct,
                attempts: r.attempts as u32,
                max_attempts: r.max_attempts as u32,
                last_error: r.last_error,
                queued_at: r.queued_at,
                started_at: r.started_at,
                finished_at: r.finished_at,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(AdminJobList { items }))
}

#[utoipa::path(
    post, path = "/admin/jobs/{id}/retry", tag = "admin",
    params(("id" = String, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 409)),
    security(("bearer" = []))
)]
pub async fn retry_job(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let job = state.jobs().by_id(&id).await?.ok_or(Error::NotFound {
        resource: format!("job:{id}"),
    })?;
    if !matches!(job.status, JobStatus::Dead | JobStatus::Failed) {
        return Err(Error::Conflict(format!(
            "job is {:?}; only dead or failed jobs can be retried",
            job.status
        ))
        .into());
    }
    // Reset attempts so the retry gets a fresh max_attempts budget; clear
    // worker_id + last_error, make it immediately eligible.
    state
        .db()
        .inner()
        .query(format!(
            r#"UPDATE job:`{id}` SET
                status = "queued",
                attempts = 0,
                worker_id = NONE,
                last_error = NONE,
                not_before = time::now(),
                started_at = NONE,
                finished_at = NONE,
                updated_at = time::now()
            "#
        ))
        .await
        .map_err(|e| Error::Database(format!("retry job: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("retry job: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// System overview
// =========================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct SystemOverview {
    pub users_total: u32,
    pub audiobooks_total: u32,
    pub chapters_total: u32,
    pub jobs_queued: u32,
    pub jobs_running: u32,
    pub jobs_completed_24h: u32,
    pub jobs_dead: u32,
    pub db_path: String,
    pub storage_path: String,
    pub storage_bytes: u64,
    pub llm_mock_mode: bool,
    pub tts_mock_mode: bool,
}

#[utoipa::path(
    get, path = "/admin/system", tag = "admin",
    responses((status = 200, body = SystemOverview), (status = 403)),
    security(("bearer" = []))
)]
pub async fn system_overview(
    State(state): State<AppState>,
    _admin: RequireAdmin,
) -> ApiResult<Json<SystemOverview>> {
    let users_total = count(&state, "user", None).await?;
    let audiobooks_total = count(&state, "audiobook", None).await?;
    let chapters_total = count(&state, "chapter", None).await?;
    let jobs_queued = count(&state, "job", Some("status = \"queued\"")).await?;
    let jobs_running = count(&state, "job", Some("status = \"running\"")).await?;
    let jobs_dead = count(&state, "job", Some("status = \"dead\"")).await?;
    let jobs_completed_24h = count(
        &state,
        "job",
        Some("status = \"completed\" AND finished_at >= time::now() - 24h"),
    )
    .await?;

    let storage_bytes = dir_size(&state.config().storage_path);
    let cfg = state.config();

    Ok(Json(SystemOverview {
        users_total,
        audiobooks_total,
        chapters_total,
        jobs_queued,
        jobs_running,
        jobs_completed_24h,
        jobs_dead,
        db_path: cfg.database_path.display().to_string(),
        storage_path: cfg.storage_path.display().to_string(),
        storage_bytes,
        llm_mock_mode: cfg.openrouter_api_key.is_empty(),
        tts_mock_mode: cfg.xai_api_key.is_empty(),
    }))
}

async fn count(state: &AppState, table: &str, filter: Option<&str>) -> Result<u32> {
    #[derive(Deserialize)]
    struct Row {
        count: i64,
    }
    let where_clause = filter.map(|f| format!("WHERE {f}")).unwrap_or_default();
    let sql = format!("SELECT count() AS count FROM {table} {where_clause} GROUP ALL");
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(sql)
        .await
        .map_err(|e| Error::Database(format!("count {table}: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("count {table} (decode): {e}")))?;
    Ok(rows.first().map(|r| r.count as u32).unwrap_or(0))
}

/// Walk a directory and sum file sizes. Used for storage_bytes in overview.
/// Bounded by the audiobook library; a nightly GC keeps this fast.
fn dir_size(path: &std::path::Path) -> u64 {
    fn walk(p: &std::path::Path, acc: &mut u64) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for entry in rd.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&entry.path(), acc);
            } else {
                *acc += meta.len();
            }
        }
    }
    let mut total = 0u64;
    walk(path, &mut total);
    total
}

// =========================================================================
// Test rigs — admin-only probes for LLMs and voices
// =========================================================================

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct TestLlmRequest {
    /// Admin LLM row id (e.g. `claude_haiku_4_5`). Required; the backend
    /// resolves the row to find the provider `model_id` to call.
    #[validate(length(min = 1, max = 120))]
    pub llm_id: String,
    #[validate(length(min = 1, max = 8000))]
    pub prompt: String,
    #[validate(length(max = 4000))]
    pub system: Option<String>,
    #[validate(range(min = 0.0, max = 2.0))]
    pub temperature: Option<f32>,
    #[validate(range(min = 1, max = 4000))]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TestLlmResponse {
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub mocked: bool,
}

#[utoipa::path(
    post, path = "/admin/test/llm", tag = "admin",
    request_body = TestLlmRequest,
    responses(
        (status = 200, body = TestLlmResponse),
        (status = 400), (status = 403), (status = 404), (status = 502),
    ),
    security(("bearer" = []))
)]
pub async fn test_llm(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Json(body): Json<TestLlmRequest>,
) -> ApiResult<Json<TestLlmResponse>> {
    body.validate().map_err(|e| Error::Validation(e.to_string()))?;
    let llm = load_llm(&state, &body.llm_id).await?;

    let mut messages: Vec<ChatMessage> = Vec::new();
    if let Some(sys) = body.system.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        messages.push(ChatMessage::system(sys));
    }
    messages.push(ChatMessage::user(body.prompt));

    let req = ChatRequest {
        model: llm.model_id,
        messages,
        temperature: body.temperature,
        max_tokens: body.max_tokens,
        json_mode: None,
        modalities: None,
    };
    let resp = state.llm().chat(&req).await?;
    Ok(Json(TestLlmResponse {
        content: resp.content,
        prompt_tokens: resp.usage.prompt_tokens,
        completion_tokens: resp.usage.completion_tokens,
        mocked: resp.mocked,
    }))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct TestVoiceRequest {
    /// Admin voice row id. The backend resolves it to `provider_voice_id`
    /// before calling the TTS provider.
    #[validate(length(min = 1, max = 120))]
    pub voice_id: String,
    #[validate(length(min = 1, max = 1000))]
    pub text: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TestVoiceResponse {
    /// WAV file (mono, 16-bit PCM) encoded as standard base64. Playable
    /// directly via `<audio src="data:audio/wav;base64,…">`.
    pub audio_wav_base64: String,
    pub sample_rate_hz: u32,
    pub duration_ms: u64,
    pub mocked: bool,
}

#[utoipa::path(
    post, path = "/admin/test/voice", tag = "admin",
    request_body = TestVoiceRequest,
    responses(
        (status = 200, body = TestVoiceResponse),
        (status = 400), (status = 403), (status = 404), (status = 502),
    ),
    security(("bearer" = []))
)]
pub async fn test_voice(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Json(body): Json<TestVoiceRequest>,
) -> ApiResult<Json<TestVoiceResponse>> {
    body.validate().map_err(|e| Error::Validation(e.to_string()))?;
    let voice = load_voice(&state, &body.voice_id).await?;
    let pcm = state
        .tts()
        .synthesize(
            &body.text,
            &voice.provider_voice_id,
            &state.config().xai_tts_language,
        )
        .await?;
    let wav = encode_wav(&pcm.samples, pcm.sample_rate_hz)?;
    let audio_wav_base64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&wav)
    };
    Ok(Json(TestVoiceResponse {
        audio_wav_base64,
        sample_rate_hz: pcm.sample_rate_hz,
        duration_ms: pcm.duration_ms(),
        mocked: pcm.mocked,
    }))
}

/// Encode PCM i16 mono to an in-memory WAV blob using the same `hound`
/// spec the on-disk chapter writer uses. Kept inline (rather than added to
/// the `audio` module) because the rest of the pipeline never needs the
/// in-memory form — on-disk WAV is the file-backed happy path.
fn encode_wav(samples: &[i16], sample_rate_hz: u32) -> Result<Vec<u8>> {
    use std::io::Cursor;

    use hound::{SampleFormat, WavSpec, WavWriter};

    let spec = WavSpec {
        channels: 1,
        sample_rate: sample_rate_hz,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut buf: Vec<u8> = Vec::with_capacity(samples.len() * 2 + 44);
    {
        let cursor = Cursor::new(&mut buf);
        let mut w = WavWriter::new(cursor, spec)
            .map_err(|e| Error::Other(anyhow::anyhow!("wav header: {e}")))?;
        for s in samples {
            w.write_sample(*s)
                .map_err(|e| Error::Other(anyhow::anyhow!("wav write: {e}")))?;
        }
        w.finalize()
            .map_err(|e| Error::Other(anyhow::anyhow!("wav finalize: {e}")))?;
    }
    Ok(buf)
}
