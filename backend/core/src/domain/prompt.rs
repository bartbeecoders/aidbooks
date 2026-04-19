use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptRole {
    Outline,
    Chapter,
    RandomTopic,
    Moderation,
    Title,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PromptTemplate {
    pub id: String,
    pub role: PromptRole,
    pub body: String,
    pub version: u32,
    pub active: bool,
    pub variables: Vec<String>,
    pub created_at: DateTime<Utc>,
}
