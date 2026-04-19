use crate::id::{AudiobookId, LlmId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::prompt::PromptRole;

/// Append-only log of every LLM call. Used for per-user quota accounting,
/// cost dashboards, and support debugging.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GenerationEvent {
    pub id: String,
    pub user: UserId,
    pub audiobook: Option<AudiobookId>,
    pub llm: LlmId,
    pub role: PromptRole,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub cost_usd: f64,
    pub success: bool,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}
