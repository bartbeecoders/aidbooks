use crate::id::{AudiobookId, JobId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Outline,
    Chapters,
    Tts,
    PostProcess,
    Cover,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Throttled,
    Dead,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub audiobook: Option<AudiobookId>,
    pub status: JobStatus,
    pub progress_pct: f32,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub queued_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub payload: Option<serde_json::Value>,
}
