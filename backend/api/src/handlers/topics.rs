//! Random-topic generator. Useful for the "Surprise me" button in the
//! create flow.

use std::collections::HashMap;

use axum::{extract::State, Json};
use listenai_core::domain::{AudiobookLength, LlmRole, PromptRole};
use listenai_core::Error;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::generation::{outline as outline_gen, prompts};
use crate::llm::{pick_llm_for_roles_lang, ChatMessage, ChatRequest};
use crate::state::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct RandomTopicRequest {
    /// Optional seed or theme hint, e.g. "sci-fi", "history of Korea".
    pub seed: Option<String>,
    /// BCP-47 language code (`"en"`, `"nl"`, …). The model writes the
    /// returned `topic` in this language, and the picker prefers an LLM
    /// whose `languages` list includes it. Defaults to English.
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RandomTopicResponse {
    pub topic: String,
    pub genre: String,
    pub length: AudiobookLength,
}

#[utoipa::path(
    post,
    path = "/topics/random",
    tag = "topics",
    request_body = RandomTopicRequest,
    responses(
        (status = 200, description = "A random audiobook topic", body = RandomTopicResponse),
        (status = 502, description = "LLM error")
    ),
    security(("bearer" = []))
)]
pub async fn random(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Json(body): Json<RandomTopicRequest>,
) -> ApiResult<Json<RandomTopicResponse>> {
    let language = body
        .language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("en");
    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert(
        "seed",
        body.seed
            .unwrap_or_else(|| "(no seed; anything goes)".into()),
    );
    vars.insert("language", crate::i18n::label(language).to_string());
    let rendered = prompts::render(&state, PromptRole::RandomTopic, &vars).await?;
    // Pick the highest-priority row tagged `default_for: ["random_topic"]`,
    // preferring rows whose `languages` matches the requested language.
    // Falls back to the chapter LLM, then to `Config.openrouter_default_model`
    // when no row matches.
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
            ChatMessage::user(rendered.body),
        ],
        temperature: Some(1.1),
        max_tokens: Some(400),
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

    // Tolerate code-fence wrapping in case a model adds it anyway.
    let cleaned = strip_code_fences(&response.content);
    let parsed: RandomTopicResponse = serde_json::from_str(cleaned)
        .map_err(|e| Error::Upstream(format!("random_topic parse: {e}")))?;
    Ok(Json(parsed))
}

fn strip_code_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}
