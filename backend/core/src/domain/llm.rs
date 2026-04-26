use crate::id::LlmId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    OpenRouter,
}

/// Where this LLM is allowed to be used by default.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmRole {
    Outline,
    Chapter,
    Title,
    RandomTopic,
    Moderation,
    /// Image-capable model used to render audiobook covers.
    CoverArt,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Llm {
    pub id: LlmId,
    pub name: String,
    pub provider: LlmProvider,
    /// Upstream model identifier (e.g. `anthropic/claude-sonnet-4.6`).
    pub model_id: String,
    pub context_window: u32,
    pub cost_prompt_per_1k: f64,
    pub cost_completion_per_1k: f64,
    pub enabled: bool,
    pub default_for: Vec<LlmRole>,
}
