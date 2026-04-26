//! Random-topic generator. Useful for the "Surprise me" button in the
//! create flow.

use std::collections::HashMap;

use axum::{extract::State, Json};
use listenai_core::domain::{AudiobookLength, PromptRole};
use listenai_core::Error;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::generation::{outline as outline_gen, prompts};
use crate::llm::{ChatMessage, ChatRequest};
use crate::state::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct RandomTopicRequest {
    /// Optional seed or theme hint, e.g. "sci-fi", "history of Korea".
    pub seed: Option<String>,
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
    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert(
        "seed",
        body.seed
            .unwrap_or_else(|| "(no seed; anything goes)".into()),
    );
    let rendered = prompts::render(&state, PromptRole::RandomTopic, &vars).await?;
    let model = state.config().openrouter_default_model.clone();

    let req = ChatRequest {
        model,
        messages: vec![
            ChatMessage::system("Respond with one JSON object only."),
            ChatMessage::user(rendered.body),
        ],
        temperature: Some(1.1),
        max_tokens: Some(400),
        json_mode: Some(true),
        modalities: None,
    };

    let response = state.llm().chat(&req).await?;
    outline_gen::log_generation_event(
        &state,
        &user.id,
        None,
        "claude_haiku_4_5",
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
