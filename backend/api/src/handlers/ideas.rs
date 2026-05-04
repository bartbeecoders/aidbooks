//! Owner-scoped audiobook idea backlog.
//!
//! Each row stores a free-form `title` plus an `audiobook_prompt` that
//! the user can ship to the create flow when they decide to turn the
//! idea into a real book. Ideas come from two places: typed manually,
//! or pulled from `POST /ideas/suggest` — an LLM-driven "what's
//! trending right now" generator that reuses the random-topic LLM
//! pick. Suggested ideas are not persisted automatically; the UI
//! shows them as preview cards and the user picks which ones to keep.
//!
//! See migration `0037_idea` for the table shape.

use chrono::{DateTime, Utc};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::generation::outline as outline_gen;
use crate::llm::{pick_llm_for_roles_lang, ChatMessage, ChatRequest};
use crate::state::AppState;
use listenai_core::domain::{LlmRole, PromptRole};

// --- DTOs ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IdeaStatus {
    Pending,
    InProgress,
    Completed,
}

impl IdeaStatus {
    fn as_str(&self) -> &'static str {
        match self {
            IdeaStatus::Pending => "pending",
            IdeaStatus::InProgress => "in_progress",
            IdeaStatus::Completed => "completed",
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IdeaRow {
    pub id: String,
    pub title: String,
    pub audiobook_prompt: String,
    pub status: IdeaStatus,
    /// `"manual"` for user-entered rows, `"trend"` for ideas created
    /// from the LLM suggestion endpoint.
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IdeaList {
    pub items: Vec<IdeaRow>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateIdeaRequest {
    #[validate(length(min = 1, max = 300))]
    pub title: String,
    #[validate(length(max = 4000))]
    pub audiobook_prompt: Option<String>,
    /// Defaults to `"manual"`. The suggestion endpoint passes
    /// `"trend"` when the user keeps an LLM-suggested row.
    #[validate(length(max = 32))]
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateIdeaRequest {
    #[validate(length(min = 1, max = 300))]
    pub title: Option<String>,
    #[validate(length(max = 4000))]
    pub audiobook_prompt: Option<String>,
    pub status: Option<IdeaStatus>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SuggestIdeasRequest {
    /// Optional theme / topic hint. Free-form — the model uses it as
    /// inspiration. Empty seeds → "broadly trending".
    #[validate(length(max = 300))]
    pub seed: Option<String>,
    /// BCP-47 code (`"en"`, `"nl"`, …) for the suggestion language.
    /// Defaults to English.
    #[validate(length(max = 16))]
    pub language: Option<String>,
    /// How many suggestions to return. Clamped to 1..=12.
    pub count: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct SuggestedIdea {
    /// Short human label (the table's "Idea" column).
    pub title: String,
    /// Full prompt the user could feed into the create flow as a topic.
    pub audiobook_prompt: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SuggestIdeasResponse {
    pub items: Vec<SuggestedIdea>,
}

// --- DB row --------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DbIdea {
    id: Thing,
    owner: Thing,
    title: String,
    #[serde(default)]
    audiobook_prompt: Option<String>,
    status: String,
    #[serde(default)]
    source: Option<String>,
    created_at: DateTime<Utc>,
    #[serde(default)]
    completed_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

impl DbIdea {
    fn to_row(&self) -> IdeaRow {
        let status = match self.status.as_str() {
            "in_progress" => IdeaStatus::InProgress,
            "completed" => IdeaStatus::Completed,
            _ => IdeaStatus::Pending,
        };
        IdeaRow {
            id: self.id.id.to_raw(),
            title: self.title.clone(),
            audiobook_prompt: self.audiobook_prompt.clone().unwrap_or_default(),
            status,
            source: self
                .source
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "manual".to_string()),
            created_at: self.created_at,
            completed_at: self.completed_at,
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
    path = "/ideas",
    tag = "ideas",
    responses(
        (status = 200, description = "Every idea owned by the authed user", body = IdeaList),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<IdeaList>> {
    let rows: Vec<DbIdea> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM idea WHERE owner = user:`{}` ORDER BY created_at DESC",
            user.id.0,
        ))
        .await
        .map_err(|e| Error::Database(format!("list ideas: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list ideas (decode): {e}")))?;

    Ok(Json(IdeaList {
        items: rows.iter().map(DbIdea::to_row).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/ideas",
    tag = "ideas",
    request_body = CreateIdeaRequest,
    responses(
        (status = 200, description = "Newly created idea", body = IdeaRow),
        (status = 400, description = "Validation failed"),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn create(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Json(body): Json<CreateIdeaRequest>,
) -> ApiResult<Json<IdeaRow>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    let id = Uuid::new_v4().simple().to_string();
    let title = body.title.trim().to_string();
    let prompt = body
        .audiobook_prompt
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let source = body
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("manual")
        .to_string();

    state
        .db()
        .inner()
        .query(format!(
            r#"CREATE idea:`{id}` CONTENT {{
                owner: user:`{uid}`,
                title: $t,
                audiobook_prompt: $p,
                source: $s,
                status: "pending"
            }}"#,
            uid = user.id.0,
        ))
        .bind(("t", title))
        .bind(("p", prompt))
        .bind(("s", source))
        .await
        .map_err(|e| Error::Database(format!("create idea: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create idea: {e}")))?;

    let row = load_owned(&state, &id, &user.id).await?;
    Ok(Json(row.to_row()))
}

#[utoipa::path(
    patch,
    path = "/ideas/{id}",
    tag = "ideas",
    params(("id" = String, Path)),
    request_body = UpdateIdeaRequest,
    responses(
        (status = 200, description = "Updated idea", body = IdeaRow),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn patch(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    Json(body): Json<UpdateIdeaRequest>,
) -> ApiResult<Json<IdeaRow>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    load_owned(&state, &id, &user.id).await?;

    if let Some(title) = body.title {
        state
            .db()
            .inner()
            .query(format!("UPDATE idea:`{id}` SET title = $t"))
            .bind(("t", title.trim().to_string()))
            .await
            .map_err(|e| Error::Database(format!("patch idea title: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch idea title: {e}")))?;
    }
    if let Some(prompt) = body.audiobook_prompt {
        state
            .db()
            .inner()
            .query(format!("UPDATE idea:`{id}` SET audiobook_prompt = $p"))
            .bind(("p", prompt.trim().to_string()))
            .await
            .map_err(|e| Error::Database(format!("patch idea prompt: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch idea prompt: {e}")))?;
    }
    if let Some(status) = body.status {
        // `completed_at` is set or cleared in lockstep with the status
        // transition so the column doesn't drift out of sync — clearing
        // it on a re-open keeps the timeline column accurate when the
        // user undoes a "mark completed" by mistake.
        let completed_clause = match status {
            IdeaStatus::Completed => "completed_at = time::now()",
            _ => "completed_at = NONE",
        };
        state
            .db()
            .inner()
            .query(format!(
                "UPDATE idea:`{id}` SET status = $st, {completed_clause}"
            ))
            .bind(("st", status.as_str().to_string()))
            .await
            .map_err(|e| Error::Database(format!("patch idea status: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch idea status: {e}")))?;
    }

    let row = load_owned(&state, &id, &user.id).await?;
    Ok(Json(row.to_row()))
}

#[utoipa::path(
    delete,
    path = "/ideas/{id}",
    tag = "ideas",
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
    load_owned(&state, &id, &user.id).await?;
    state
        .db()
        .inner()
        .query(format!("DELETE idea:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("delete idea: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("delete idea: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/ideas/suggest",
    tag = "ideas",
    request_body = SuggestIdeasRequest,
    responses(
        (status = 200, description = "LLM-generated idea suggestions", body = SuggestIdeasResponse),
        (status = 400, description = "Validation failed"),
        (status = 502, description = "LLM error")
    ),
    security(("bearer" = []))
)]
pub async fn suggest(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Json(body): Json<SuggestIdeasRequest>,
) -> ApiResult<Json<SuggestIdeasResponse>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    let language = body
        .language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("en");
    let count = body.count.unwrap_or(8).clamp(1, 12);
    let seed = body
        .seed
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("(no seed; surprise me with whatever's currently capturing public curiosity)");

    let lang_label = crate::i18n::label(language);
    let prompt = format!(
        "You are a research assistant who tracks what's currently going viral on \
X (Twitter), Reddit, TikTok, Hacker News and major news cycles. Suggest {count} \
fresh audiobook ideas a creator could turn into a 30–90 minute listen RIGHT NOW \
to ride trending interest. Bias towards specific stories/angles over broad \
genres.\n\n\
Theme hint: {seed}\n\
Output language: {lang_label}\n\n\
Output rules:\n\
- Respond with a SINGLE JSON object and nothing else.\n\
- Do not wrap the JSON in markdown code fences.\n\n\
Required shape:\n\
{{\n\
  \"items\": [\n\
    {{\n\
      \"title\": \"<short label, 4–10 words>\",\n\
      \"audiobook_prompt\": \"<one paragraph, 2–4 sentences, framing the topic \
as an audiobook brief — angle, hook, why it matters now>\"\n\
    }}\n\
  ]\n\
}}\n\n\
Guidelines:\n\
- Write `title` and `audiobook_prompt` in the requested output language.\n\
- Prefer concrete, specific topics tied to current events or recent trends.\n\
- Spread across genres: history, science, technology, culture, business.\n\
- Avoid celebrity gossip, real crimes involving named individuals, or hateful framings.\n\
- Each idea should be distinct — no near-duplicates."
    );

    let picked = pick_llm_for_roles_lang(
        &state,
        &[LlmRole::RandomTopic, LlmRole::Chapter],
        Some(language),
    )
    .await?;

    let req = ChatRequest {
        model: picked.model_id.clone(),
        messages: vec![
            ChatMessage::system("Respond with one JSON object only."),
            ChatMessage::user(prompt),
        ],
        temperature: Some(1.0),
        max_tokens: Some(2000),
        json_mode: Some(true),
        modalities: None,
        provider: Some(picked.provider.clone()),
    };

    let response = state.llm().chat(&req).await?;
    outline_gen::log_generation_event(
        &state,
        &user.id,
        None,
        &picked.llm_id,
        PromptRole::RandomTopic,
        &response,
        None,
    )
    .await?;

    #[derive(Deserialize)]
    struct Wrapper {
        items: Vec<SuggestedIdea>,
    }
    let cleaned = strip_code_fences(&response.content);
    let parsed: Wrapper = serde_json::from_str(cleaned)
        .map_err(|e| Error::Upstream(format!("idea suggest parse: {e}")))?;
    Ok(Json(SuggestIdeasResponse {
        items: parsed.items,
    }))
}

// --- Helpers -------------------------------------------------------------

async fn load_owned(state: &AppState, id: &str, user: &UserId) -> Result<DbIdea> {
    let rows: Vec<DbIdea> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM idea:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("load idea: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load idea (decode): {e}")))?;
    let row = rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("idea:{id}"),
    })?;
    if row.owner_id() != *user {
        return Err(Error::NotFound {
            resource: format!("idea:{id}"),
        });
    }
    Ok(row)
}

fn strip_code_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}
