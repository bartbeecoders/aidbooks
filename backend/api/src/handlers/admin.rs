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
// OpenRouter model catalog (used by the LLM admin picker)
// =========================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct OpenRouterModelRow {
    pub id: String,
    pub name: String,
    /// Optional one-line description; trimmed to 200 chars on the frontend.
    pub description: Option<String>,
    pub context_length: Option<u32>,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    /// USD per 1k prompt tokens (converted from OpenRouter's per-token).
    pub cost_prompt_per_1k: f64,
    /// USD per 1k completion tokens.
    pub cost_completion_per_1k: f64,
    /// USD per generated image (only set on image-output models). Most
    /// providers price one ~1MP frame, so the admin UI uses this directly
    /// as the `$ / megapixel` default.
    pub cost_per_image: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpenRouterModelList {
    pub items: Vec<OpenRouterModelRow>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OpenRouterModelsQuery {
    /// Filter forwarded as `?output_modalities=` to OpenRouter. Pass
    /// `image` to get the full image-generation catalog — the unfiltered
    /// endpoint hides most image-only providers.
    #[serde(default)]
    pub output_modalities: Option<String>,
}

// =========================================================================
// xAI model catalog (used by the LLM admin picker, xAI tab)
// =========================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct XaiModelRow {
    /// Upstream model id (e.g. `grok-4`).
    pub id: String,
    pub aliases: Vec<String>,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    /// USD per 1k prompt tokens — converted from xAI's
    /// microdollars-per-million-tokens encoding.
    pub cost_prompt_per_1k: f64,
    pub cost_completion_per_1k: f64,
    /// Max prompt window in tokens, when reported by xAI.
    pub context_length: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct XaiModelList {
    pub items: Vec<XaiModelRow>,
}

#[utoipa::path(
    get, path = "/admin/xai/models", tag = "admin",
    responses(
        (status = 200, body = XaiModelList),
        (status = 400, description = "xai_api_key not configured"),
        (status = 403),
        (status = 502, description = "xAI unreachable")
    ),
    security(("bearer" = []))
)]
pub async fn list_xai_models(
    State(state): State<AppState>,
    _admin: RequireAdmin,
) -> ApiResult<Json<XaiModelList>> {
    let models = state.llm().list_xai_models().await?;
    let items = models
        .into_iter()
        .map(|m| {
            // xAI: integer microdollars per million tokens.
            // USD per 1k = price / 1_000_000_000.
            let to_per_1k =
                |v: Option<u64>| -> f64 { v.map(|n| (n as f64) / 1_000_000_000.0).unwrap_or(0.0) };
            XaiModelRow {
                id: m.id,
                aliases: m.aliases,
                input_modalities: m.input_modalities,
                output_modalities: m.output_modalities,
                cost_prompt_per_1k: to_per_1k(m.prompt_text_token_price),
                cost_completion_per_1k: to_per_1k(m.completion_text_token_price),
                context_length: m.max_prompt_length.map(|n| n.min(u32::MAX as u64) as u32),
            }
        })
        .collect();
    Ok(Json(XaiModelList { items }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct XaiImageModelRow {
    pub id: String,
    pub aliases: Vec<String>,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    /// USD per generated image — converted from xAI's microdollars-per-image
    /// encoding. The image admin form uses this as the `$ / megapixel`
    /// pre-fill; xAI image models bill per image, not per pixel.
    pub cost_per_image: f64,
    pub context_length: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct XaiImageModelList {
    pub items: Vec<XaiImageModelRow>,
}

#[utoipa::path(
    get, path = "/admin/xai/image-models", tag = "admin",
    responses(
        (status = 200, body = XaiImageModelList),
        (status = 400, description = "xai_api_key not configured"),
        (status = 403),
        (status = 502, description = "xAI unreachable")
    ),
    security(("bearer" = []))
)]
pub async fn list_xai_image_models(
    State(state): State<AppState>,
    _admin: RequireAdmin,
) -> ApiResult<Json<XaiImageModelList>> {
    let models = state.llm().list_xai_image_models().await?;
    let items = models
        .into_iter()
        .map(|m| XaiImageModelRow {
            id: m.id,
            aliases: m.aliases,
            input_modalities: m.input_modalities,
            output_modalities: m.output_modalities,
            cost_per_image: m
                .image_generation_price
                .map(|n| (n as f64) / 1_000_000.0)
                .unwrap_or(0.0),
            context_length: m.max_prompt_length.map(|n| n.min(u32::MAX as u64) as u32),
        })
        .collect();
    Ok(Json(XaiImageModelList { items }))
}

#[utoipa::path(
    get, path = "/admin/openrouter/models", tag = "admin",
    params(
        ("output_modalities" = Option<String>, Query,
            description = "Filter forwarded to OpenRouter (e.g. 'image').")
    ),
    responses(
        (status = 200, body = OpenRouterModelList),
        (status = 403),
        (status = 502, description = "OpenRouter unreachable")
    ),
    security(("bearer" = []))
)]
pub async fn list_openrouter_models(
    State(state): State<AppState>,
    Query(q): Query<OpenRouterModelsQuery>,
    _admin: RequireAdmin,
) -> ApiResult<Json<OpenRouterModelList>> {
    let models = state
        .llm()
        .list_openrouter_models(q.output_modalities.as_deref())
        .await?;
    let items = models
        .into_iter()
        .map(|m| {
            // Pricing strings are USD per token; multiply by 1000 to land in
            // the same unit our `cost_*_per_1k` columns use. Per-image price
            // we keep as-is.
            let parse = |s: Option<String>| -> f64 {
                s.as_deref()
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0)
            };
            let pricing = m.pricing.unwrap_or_default();
            let arch = m.architecture.unwrap_or_default();
            OpenRouterModelRow {
                id: m.id,
                name: m.name.unwrap_or_default(),
                description: m.description,
                context_length: m.context_length.map(|n| n.min(u32::MAX as u64) as u32),
                input_modalities: arch.input_modalities,
                output_modalities: arch.output_modalities,
                cost_prompt_per_1k: parse(pricing.prompt) * 1000.0,
                cost_completion_per_1k: parse(pricing.completion) * 1000.0,
                cost_per_image: parse(pricing.image),
            }
        })
        .collect();
    Ok(Json(OpenRouterModelList { items }))
}

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
    /// Per-megapixel price for image generation models. `0` for text models.
    #[serde(default)]
    pub cost_per_megapixel: f64,
    pub enabled: bool,
    pub default_for: Vec<String>,
    /// What kind of model this is (`text`, `image`, …). `None` ⇒ unspecified.
    pub function: Option<String>,
    /// BCP-47 codes this model handles well. Empty = any language.
    pub languages: Vec<String>,
    /// Picker tiebreaker; lower wins.
    pub priority: i32,
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
    /// Upstream model id. Allow renaming (e.g. tracking a vendor's slug
    /// changes) without delete-and-recreate.
    #[validate(length(min = 1, max = 200))]
    pub model_id: Option<String>,
    #[validate(range(min = 1, max = 10_000_000))]
    pub context_window: Option<u32>,
    #[validate(range(min = 0.0, max = 1000.0))]
    pub cost_prompt_per_1k: Option<f64>,
    #[validate(range(min = 0.0, max = 1000.0))]
    pub cost_completion_per_1k: Option<f64>,
    /// Per-megapixel price for image models.
    #[validate(range(min = 0.0, max = 1000.0))]
    pub cost_per_megapixel: Option<f64>,
    pub default_for: Option<Vec<String>>,
    /// `Some("")` clears the function; omitted leaves it unchanged.
    #[validate(length(max = 40))]
    pub function: Option<String>,
    /// Replaces the language list wholesale. Pass `[]` to mean "any".
    pub languages: Option<Vec<String>>,
    #[validate(range(min = 0, max = 1_000_000))]
    pub priority: Option<i32>,
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
    /// Per-megapixel price for image models. Defaults to 0 if omitted.
    #[serde(default)]
    #[validate(range(min = 0.0, max = 1000.0))]
    pub cost_per_megapixel: Option<f64>,
    pub enabled: Option<bool>,
    pub default_for: Option<Vec<String>>,
    #[validate(length(max = 40))]
    pub function: Option<String>,
    pub languages: Option<Vec<String>>,
    #[validate(range(min = 0, max = 1_000_000))]
    pub priority: Option<i32>,
    /// Wire identifier for the upstream provider (`open_router` | `xai`).
    /// Defaults to `open_router` when omitted, matching legacy clients.
    #[serde(default)]
    #[validate(length(max = 32))]
    pub provider: Option<String>,
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
    #[serde(default)]
    cost_per_megapixel: f64,
    enabled: bool,
    default_for: Vec<String>,
    #[serde(default)]
    function: Option<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default = "default_priority")]
    priority: i64,
}

fn default_priority() -> i64 {
    100
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
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    let mut sets: Vec<&str> = Vec::new();
    if body.enabled.is_some() {
        sets.push("enabled = $enabled");
    }
    if body.name.is_some() {
        sets.push("name = $name");
    }
    if body.model_id.is_some() {
        sets.push("model_id = $model_id");
    }
    if body.context_window.is_some() {
        sets.push("context_window = $cw");
    }
    if body.cost_prompt_per_1k.is_some() {
        sets.push("cost_prompt_per_1k = $cp");
    }
    if body.cost_completion_per_1k.is_some() {
        sets.push("cost_completion_per_1k = $cc");
    }
    if body.cost_per_megapixel.is_some() {
        sets.push("cost_per_megapixel = $cmp");
    }
    if body.default_for.is_some() {
        sets.push("default_for = $df");
    }
    if body.function.is_some() {
        sets.push("function = $function");
    }
    if body.languages.is_some() {
        sets.push("languages = $languages");
    }
    if body.priority.is_some() {
        sets.push("priority = $priority");
    }
    if sets.is_empty() {
        return Err(Error::Validation("no fields to update".into()).into());
    }

    // Empty-string on `function` → NONE so admins can clear the value.
    let function_arg = body.function.map(|s| {
        let t = s.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });

    let sql = format!("UPDATE llm:`{id}` SET {}", sets.join(", "));
    state
        .db()
        .inner()
        .query(sql)
        .bind(("enabled", body.enabled))
        .bind(("name", body.name))
        .bind(("model_id", body.model_id))
        .bind(("cw", body.context_window.map(|n| n as i64)))
        .bind(("cp", body.cost_prompt_per_1k))
        .bind(("cc", body.cost_completion_per_1k))
        .bind(("cmp", body.cost_per_megapixel))
        .bind(("df", body.default_for))
        .bind(("function", function_arg))
        .bind(("languages", body.languages))
        .bind(("priority", body.priority.map(|n| n as i64)))
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
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
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
    let function = body
        .function
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| Some("text".to_string()));
    let languages = body.languages.unwrap_or_default();
    let priority = body.priority.unwrap_or(100) as i64;
    let provider = match body
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some("open_router") | None => "open_router".to_string(),
        Some("xai") => "xai".to_string(),
        Some(other) => {
            return Err(Error::Validation(format!("unknown provider `{other}`")).into());
        }
    };

    state
        .db()
        .inner()
        .query(format!(
            r#"CREATE llm:`{}` CONTENT {{
                name: $name,
                provider: $provider,
                model_id: $model_id,
                context_window: $cw,
                cost_prompt_per_1k: $cp,
                cost_completion_per_1k: $cc,
                cost_per_megapixel: $cmp,
                enabled: $enabled,
                default_for: $df,
                function: $function,
                languages: $languages,
                priority: $priority
            }}"#,
            body.id
        ))
        .bind(("name", body.name))
        .bind(("provider", provider))
        .bind(("model_id", body.model_id))
        .bind(("cw", body.context_window as i64))
        .bind(("cp", body.cost_prompt_per_1k))
        .bind(("cc", body.cost_completion_per_1k))
        .bind(("cmp", body.cost_per_megapixel.unwrap_or(0.0)))
        .bind(("enabled", enabled))
        .bind(("df", default_for))
        .bind(("function", function))
        .bind(("languages", languages))
        .bind(("priority", priority))
        .await
        .map_err(|e| Error::Database(format!("create_llm: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create_llm: {e}")))?;

    Ok((StatusCode::CREATED, Json(load_llm(&state, &body.id).await?)))
}

#[utoipa::path(
    delete, path = "/admin/llm/{id}", tag = "admin",
    params(("id" = String, Path)),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found"),
        (status = 403, description = "Not an admin")
    ),
    security(("bearer" = []))
)]
pub async fn delete_llm(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if !is_valid_llm_id(&id) {
        return Err(Error::Validation("invalid llm id".into()).into());
    }
    state
        .db()
        .inner()
        .query(format!("DELETE llm:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("delete_llm: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("delete_llm: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
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
    rows.into_iter()
        .next()
        .map(row_to_llm)
        .ok_or(Error::NotFound {
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
        cost_per_megapixel: r.cost_per_megapixel,
        enabled: r.enabled,
        default_for: r.default_for,
        function: r.function.filter(|s| !s.trim().is_empty()),
        languages: r.languages,
        priority: r.priority as i32,
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
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
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
    rows.into_iter()
        .next()
        .map(row_to_voice)
        .ok_or(Error::NotFound {
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
    let sql = format!("SELECT * FROM user {where_clause} ORDER BY created_at DESC LIMIT {limit}",);

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
    let sql = format!("SELECT * FROM job {where_clause} ORDER BY queued_at DESC LIMIT {limit}");
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

/// Cancel a job that hasn't reached a terminal state yet. Flips the row to
/// `dead` so the worker pool stops considering it; for in-flight jobs the
/// worker keeps running until its current chunk finishes, but its terminal
/// write becomes a no-op (gated on `status = running` in the repo) so the
/// cancel sticks. Already-terminal jobs return 409 — use delete instead.
#[utoipa::path(
    post, path = "/admin/jobs/{id}/cancel", tag = "admin",
    params(("id" = String, Path)),
    responses(
        (status = 204, description = "Cancelled"),
        (status = 403),
        (status = 404),
        (status = 409, description = "Job is already in a terminal state")
    ),
    security(("bearer" = []))
)]
pub async fn cancel_job(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let job = state.jobs().by_id(&id).await?.ok_or(Error::NotFound {
        resource: format!("job:{id}"),
    })?;
    if job.status.is_terminal() {
        return Err(Error::Conflict(format!(
            "job is {:?}; only queued/running/throttled/failed jobs can be cancelled",
            job.status
        ))
        .into());
    }
    state
        .db()
        .inner()
        .query(format!(
            r#"UPDATE job:`{id}` SET
                status = "dead",
                finished_at = time::now(),
                updated_at = time::now(),
                last_error = "cancelled by admin",
                worker_id = NONE
            "#
        ))
        .await
        .map_err(|e| Error::Database(format!("cancel job: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("cancel job: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Permanently delete a job row. Also deletes any direct children
/// (`parent = job:<id>`) so a parent fan-out doesn't leave orphans.
#[utoipa::path(
    delete, path = "/admin/jobs/{id}", tag = "admin",
    params(("id" = String, Path)),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403),
        (status = 404)
    ),
    security(("bearer" = []))
)]
pub async fn delete_job(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if !is_safe_job_id(&id) {
        return Err(Error::Validation("invalid job id".into()).into());
    }
    state
        .db()
        .inner()
        .query(format!(
            "DELETE job WHERE parent = job:`{id}`; DELETE job:`{id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("delete job: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("delete job: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Job ids are uuid simple (32 hex chars) but we accept any safe alphanumeric
/// to stay tolerant of older formats. Whitelist embedded path so it can't
/// inject SurrealQL.
fn is_safe_job_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
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
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    let llm = load_llm(&state, &body.llm_id).await?;

    let mut messages: Vec<ChatMessage> = Vec::new();
    if let Some(sys) = body
        .system
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
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
        provider: Some(llm.provider),
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
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
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

// =========================================================================
// YouTube description footers (per-language disclaimer + backlink)
// =========================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct YoutubeFooterRow {
    /// BCP-47 language code (`en`, `nl`, `fr`, …). Doubles as the record
    /// id in the `youtube_description_footer` table.
    pub language: String,
    pub text: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct YoutubeFooterList {
    pub items: Vec<YoutubeFooterRow>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpsertYoutubeFooterRequest {
    /// Body of the footer. Appended verbatim after a blank line at the
    /// end of every YouTube description for this language. Capped at
    /// 4000 chars so it never crowds out the auto-generated chapters
    /// list inside YouTube's 5000-char description ceiling.
    #[validate(length(min = 1, max = 4000))]
    pub text: String,
}

#[utoipa::path(
    get, path = "/admin/youtube-settings", tag = "admin",
    responses(
        (status = 200, body = YoutubeFooterList),
        (status = 403)
    ),
    security(("bearer" = []))
)]
pub async fn list_youtube_footers(
    State(state): State<AppState>,
    _admin: RequireAdmin,
) -> ApiResult<Json<YoutubeFooterList>> {
    #[derive(Deserialize)]
    struct Row {
        id: Thing,
        text: String,
        updated_at: DateTime<Utc>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query("SELECT id, text, updated_at FROM youtube_description_footer ORDER BY id ASC")
        .await
        .map_err(|e| Error::Database(format!("list footers: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list footers (decode): {e}")))?;
    let items = rows
        .into_iter()
        .map(|r| YoutubeFooterRow {
            language: r.id.id.to_raw(),
            text: r.text,
            updated_at: r.updated_at,
        })
        .collect();
    Ok(Json(YoutubeFooterList { items }))
}

#[utoipa::path(
    put, path = "/admin/youtube-settings/{language}", tag = "admin",
    params(("language" = String, Path, description = "BCP-47 language code")),
    request_body = UpsertYoutubeFooterRequest,
    responses(
        (status = 200, body = YoutubeFooterRow),
        (status = 400, description = "Validation failed"),
        (status = 403)
    ),
    security(("bearer" = []))
)]
pub async fn upsert_youtube_footer(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(language): Path<String>,
    Json(body): Json<UpsertYoutubeFooterRequest>,
) -> ApiResult<Json<YoutubeFooterRow>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    let lang = language.trim();
    if !is_valid_lang(lang) {
        return Err(Error::Validation("invalid language code".into()).into());
    }
    state
        .db()
        .inner()
        .query(format!(
            "UPSERT youtube_description_footer:`{lang}` MERGE {{ \
                text: $text, updated_at: time::now() \
            }}"
        ))
        .bind(("text", body.text.clone()))
        .await
        .map_err(|e| Error::Database(format!("upsert footer: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("upsert footer: {e}")))?;
    #[derive(Deserialize)]
    struct Row {
        text: String,
        updated_at: DateTime<Utc>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT text, updated_at FROM youtube_description_footer:`{lang}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("read footer: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("read footer (decode): {e}")))?;
    let row = rows
        .into_iter()
        .next()
        .ok_or_else(|| Error::Database("upserted footer not readable".into()))?;
    Ok(Json(YoutubeFooterRow {
        language: lang.to_string(),
        text: row.text,
        updated_at: row.updated_at,
    }))
}

#[utoipa::path(
    delete, path = "/admin/youtube-settings/{language}", tag = "admin",
    params(("language" = String, Path)),
    responses(
        (status = 204),
        (status = 403)
    ),
    security(("bearer" = []))
)]
pub async fn delete_youtube_footer(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(language): Path<String>,
) -> ApiResult<StatusCode> {
    let lang = language.trim();
    if !is_valid_lang(lang) {
        return Err(Error::Validation("invalid language code".into()).into());
    }
    state
        .db()
        .inner()
        .query(format!("DELETE youtube_description_footer:`{lang}`"))
        .await
        .map_err(|e| Error::Database(format!("delete footer: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("delete footer: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

// =========================================================================
// YouTube publish settings (singleton — currently just the credits toggle)
// =========================================================================

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct YoutubePublishSettings {
    /// When true, the publisher appends a "Models used:" block to every
    /// YouTube description it builds, reflecting the LLMs/voices that
    /// actually contributed to that audiobook (read from
    /// `generation_event`). Off by default — opt-in flag.
    pub include_credits: bool,
    /// When true, the encoder burns a "👍 Like & Subscribe!" overlay
    /// into every newly-encoded YouTube video for two short windows
    /// (a few seconds in and again before the end). Off by default —
    /// opt-in flag.
    #[serde(default)]
    pub like_subscribe_overlay: bool,
}

#[utoipa::path(
    get, path = "/admin/youtube-publish-settings", tag = "admin",
    responses((status = 200, body = YoutubePublishSettings), (status = 403)),
    security(("bearer" = []))
)]
pub async fn get_youtube_publish_settings(
    State(state): State<AppState>,
    _admin: RequireAdmin,
) -> ApiResult<Json<YoutubePublishSettings>> {
    Ok(Json(load_youtube_publish_settings(&state).await?))
}

#[utoipa::path(
    put, path = "/admin/youtube-publish-settings", tag = "admin",
    request_body = YoutubePublishSettings,
    responses((status = 200, body = YoutubePublishSettings), (status = 403)),
    security(("bearer" = []))
)]
pub async fn put_youtube_publish_settings(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Json(body): Json<YoutubePublishSettings>,
) -> ApiResult<Json<YoutubePublishSettings>> {
    state
        .db()
        .inner()
        .query(
            "UPSERT youtube_publish_settings:singleton MERGE { \
                include_credits: $ic, like_subscribe_overlay: $ls, updated_at: time::now() \
            }",
        )
        .bind(("ic", body.include_credits))
        .bind(("ls", body.like_subscribe_overlay))
        .await
        .map_err(|e| Error::Database(format!("upsert publish settings: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("upsert publish settings: {e}")))?;
    Ok(Json(load_youtube_publish_settings(&state).await?))
}

/// Public helper so the YouTube publisher can read the same singleton
/// without going through the admin route.
pub async fn load_youtube_publish_settings(state: &AppState) -> Result<YoutubePublishSettings> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        include_credits: bool,
        #[serde(default)]
        like_subscribe_overlay: bool,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(
            "SELECT include_credits, like_subscribe_overlay \
             FROM youtube_publish_settings:singleton",
        )
        .await
        .map_err(|e| Error::Database(format!("read publish settings: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("read publish settings (decode): {e}")))?;
    let row = rows.into_iter().next();
    Ok(YoutubePublishSettings {
        include_credits: row.as_ref().map(|r| r.include_credits).unwrap_or(false),
        like_subscribe_overlay: row.map(|r| r.like_subscribe_overlay).unwrap_or(false),
    })
}

/// BCP-47-ish allowlist for the footer record id. We embed the language
/// in `youtube_description_footer:`<lang>`` so the charset has to stay
/// SurrealDB-safe. ASCII letters, digits, and `-` cover every BCP-47
/// code we'd realistically ship.
fn is_valid_lang(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

// =========================================================================
// Audiobook category catalog (admin-curated)
// =========================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookCategoryRow {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Number of audiobooks currently using this category.
    pub usage_count: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookCategoryList {
    pub items: Vec<AudiobookCategoryRow>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateAudiobookCategoryRequest {
    #[validate(length(min = 1, max = 60))]
    pub name: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateAudiobookCategoryRequest {
    #[validate(length(min = 1, max = 60))]
    pub name: String,
}

#[utoipa::path(
    get, path = "/admin/audiobook-categories", tag = "admin",
    responses(
        (status = 200, body = AudiobookCategoryList),
        (status = 403)
    ),
    security(("bearer" = []))
)]
pub async fn list_audiobook_categories(
    State(state): State<AppState>,
    _admin: RequireAdmin,
) -> ApiResult<Json<AudiobookCategoryList>> {
    Ok(Json(AudiobookCategoryList {
        items: load_categories(&state).await?,
    }))
}

#[utoipa::path(
    post, path = "/admin/audiobook-categories", tag = "admin",
    request_body = CreateAudiobookCategoryRequest,
    responses(
        (status = 201, body = AudiobookCategoryRow),
        (status = 400, description = "Validation failed"),
        (status = 409, description = "Name already exists"),
        (status = 403)
    ),
    security(("bearer" = []))
)]
pub async fn create_audiobook_category(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Json(body): Json<CreateAudiobookCategoryRequest>,
) -> ApiResult<(StatusCode, Json<AudiobookCategoryRow>)> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(Error::Validation("name must not be empty".into()).into());
    }
    if category_exists(&state, &name).await? {
        return Err(Error::Conflict(format!("category `{name}` already exists")).into());
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    state
        .db()
        .inner()
        .query(format!(
            r#"CREATE audiobook_category:`{id}` CONTENT {{
                name: $name,
                created_at: time::now(),
                updated_at: time::now()
            }}"#
        ))
        .bind(("name", name.clone()))
        .await
        .map_err(|e| Error::Database(format!("create category: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create category: {e}")))?;
    let row = read_category(&state, &id).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

#[utoipa::path(
    patch, path = "/admin/audiobook-categories/{id}", tag = "admin",
    params(("id" = String, Path)),
    request_body = UpdateAudiobookCategoryRequest,
    responses(
        (status = 200, body = AudiobookCategoryRow),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Name already exists"),
        (status = 403)
    ),
    security(("bearer" = []))
)]
pub async fn update_audiobook_category(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateAudiobookCategoryRequest>,
) -> ApiResult<Json<AudiobookCategoryRow>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    if !is_safe_record_id(&id) {
        return Err(Error::Validation("invalid id".into()).into());
    }
    let new_name = body.name.trim().to_string();
    if new_name.is_empty() {
        return Err(Error::Validation("name must not be empty".into()).into());
    }
    let old_name = read_category_name(&state, &id).await?;
    if old_name == new_name {
        return Ok(Json(read_category(&state, &id).await?));
    }
    if category_exists(&state, &new_name).await? {
        return Err(Error::Conflict(format!("category `{new_name}` already exists")).into());
    }
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook_category:`{id}` SET name = $name, updated_at = time::now()"
        ))
        .bind(("name", new_name.clone()))
        .await
        .map_err(|e| Error::Database(format!("rename category: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("rename category: {e}")))?;
    // Cascade: every audiobook that referenced the old name moves to the
    // new one. Without this, a rename would silently orphan books.
    state
        .db()
        .inner()
        .query("UPDATE audiobook SET category = $new WHERE category = $old")
        .bind(("old", old_name))
        .bind(("new", new_name.clone()))
        .await
        .map_err(|e| Error::Database(format!("cascade category rename: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("cascade category rename: {e}")))?;
    Ok(Json(read_category(&state, &id).await?))
}

#[utoipa::path(
    delete, path = "/admin/audiobook-categories/{id}", tag = "admin",
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 404, description = "Not found"),
        (status = 403)
    ),
    security(("bearer" = []))
)]
pub async fn delete_audiobook_category(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if !is_safe_record_id(&id) {
        return Err(Error::Validation("invalid id".into()).into());
    }
    let name = read_category_name(&state, &id).await?;
    // Cascade: clear `category` on any audiobook that referenced this row
    // — they move to "Uncategorized" rather than vanishing from the view.
    state
        .db()
        .inner()
        .query("UPDATE audiobook SET category = NONE WHERE category = $name")
        .bind(("name", name))
        .await
        .map_err(|e| Error::Database(format!("cascade category delete: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("cascade category delete: {e}")))?;
    state
        .db()
        .inner()
        .query(format!("DELETE audiobook_category:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("delete category: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("delete category: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Helper: load all categories with usage counts.
pub(crate) async fn load_categories(state: &AppState) -> Result<Vec<AudiobookCategoryRow>> {
    #[derive(Deserialize)]
    struct Row {
        id: Thing,
        name: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query("SELECT id, name, created_at, updated_at FROM audiobook_category ORDER BY name ASC")
        .await
        .map_err(|e| Error::Database(format!("list categories: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list categories (decode): {e}")))?;

    // One pass over `audiobook` to count usage per category. Aggregating
    // in Rust beats running N counts when the catalog has a handful of
    // entries; the table is small.
    #[derive(Deserialize)]
    struct UsageRow {
        category: Option<String>,
    }
    let usage_rows: Vec<UsageRow> = state
        .db()
        .inner()
        .query("SELECT category FROM audiobook")
        .await
        .map_err(|e| Error::Database(format!("count categories: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("count categories (decode): {e}")))?;
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for u in usage_rows {
        if let Some(c) = u
            .category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            *counts.entry(c.to_string()).or_default() += 1;
        }
    }

    Ok(rows
        .into_iter()
        .map(|r| {
            let id = r.id.id.to_raw();
            let usage_count = counts.get(&r.name).copied().unwrap_or(0);
            AudiobookCategoryRow {
                id,
                name: r.name,
                created_at: r.created_at,
                updated_at: r.updated_at,
                usage_count,
            }
        })
        .collect())
}

async fn category_exists(state: &AppState, name: &str) -> Result<bool> {
    #[derive(Deserialize)]
    struct Row {
        #[allow(dead_code)]
        name: String,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query("SELECT name FROM audiobook_category WHERE name = $n LIMIT 1")
        .bind(("n", name.to_string()))
        .await
        .map_err(|e| Error::Database(format!("category exists: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("category exists (decode): {e}")))?;
    Ok(!rows.is_empty())
}

async fn read_category(state: &AppState, id: &str) -> Result<AudiobookCategoryRow> {
    let all = load_categories(state).await?;
    all.into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| Error::NotFound {
            resource: format!("audiobook_category:{id}"),
        })
}

async fn read_category_name(state: &AppState, id: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!("SELECT name FROM audiobook_category:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("read category: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("read category (decode): {e}")))?;
    rows.into_iter()
        .next()
        .map(|r| r.name)
        .ok_or_else(|| Error::NotFound {
            resource: format!("audiobook_category:{id}"),
        })
}

/// SurrealDB record-id charset filter — matches what `is_valid_llm_id`
/// uses elsewhere. Keeps embedded `audiobook_category:`<id>`` injection-safe.
fn is_safe_record_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
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
