//! Read-only catalogues for the create-flow UI: available voices and LLMs.
//! Admin-write endpoints will land in Phase 7.

use axum::{extract::State, Json};
use listenai_core::domain::{Llm, LlmProvider, LlmRole, Voice, VoiceGender};
use listenai_core::id::{LlmId, VoiceId};
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use utoipa::ToSchema;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct VoiceList {
    pub items: Vec<Voice>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LlmList {
    pub items: Vec<Llm>,
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
    get,
    path = "/voices",
    tag = "catalog",
    responses(
        (status = 200, description = "Enabled voices", body = VoiceList),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list_voices(
    State(state): State<AppState>,
    Authenticated(_user): Authenticated,
) -> ApiResult<Json<VoiceList>> {
    let rows: Vec<DbVoice> = state
        .db()
        .inner()
        .query("SELECT * FROM voice WHERE enabled = true ORDER BY name ASC")
        .await
        .map_err(|e| Error::Database(format!("list voices: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list voices (decode): {e}")))?;

    let items = rows
        .into_iter()
        .map(|r| {
            Ok(Voice {
                id: VoiceId(r.id.id.to_raw()),
                name: r.name,
                provider: r.provider,
                provider_voice_id: r.provider_voice_id,
                gender: parse_gender(&r.gender)?,
                accent: r.accent,
                language: r.language,
                sample_url: r.sample_url,
                enabled: r.enabled,
                premium_only: r.premium_only,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(VoiceList { items }))
}

#[utoipa::path(
    get,
    path = "/llms",
    tag = "catalog",
    responses(
        (status = 200, description = "Enabled LLM configs", body = LlmList),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list_llms(
    State(state): State<AppState>,
    Authenticated(_user): Authenticated,
) -> ApiResult<Json<LlmList>> {
    let rows: Vec<DbLlm> = state
        .db()
        .inner()
        .query("SELECT * FROM llm WHERE enabled = true ORDER BY name ASC")
        .await
        .map_err(|e| Error::Database(format!("list llms: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list llms (decode): {e}")))?;

    let items = rows
        .into_iter()
        .map(|r| {
            Ok(Llm {
                id: LlmId(r.id.id.to_raw()),
                name: r.name,
                provider: parse_provider(&r.provider)?,
                model_id: r.model_id,
                context_window: r.context_window as u32,
                cost_prompt_per_1k: r.cost_prompt_per_1k,
                cost_completion_per_1k: r.cost_completion_per_1k,
                cost_per_megapixel: r.cost_per_megapixel,
                enabled: r.enabled,
                default_for: r.default_for.iter().filter_map(|s| parse_role(s)).collect(),
                function: r.function.filter(|s| !s.trim().is_empty()),
                languages: r.languages,
                priority: r.priority as i32,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(LlmList { items }))
}

fn parse_gender(s: &str) -> Result<VoiceGender> {
    Ok(match s {
        "female" => VoiceGender::Female,
        "male" => VoiceGender::Male,
        "neutral" => VoiceGender::Neutral,
        other => return Err(Error::Database(format!("unknown gender `{other}`"))),
    })
}

fn parse_provider(s: &str) -> Result<LlmProvider> {
    Ok(match s {
        "open_router" => LlmProvider::OpenRouter,
        "xai" => LlmProvider::Xai,
        other => return Err(Error::Database(format!("unknown provider `{other}`"))),
    })
}

fn parse_role(s: &str) -> Option<LlmRole> {
    Some(match s {
        "outline" => LlmRole::Outline,
        "chapter" => LlmRole::Chapter,
        "title" => LlmRole::Title,
        "random_topic" => LlmRole::RandomTopic,
        "moderation" => LlmRole::Moderation,
        "cover_art" => LlmRole::CoverArt,
        "translate" => LlmRole::Translate,
        _ => return None,
    })
}
