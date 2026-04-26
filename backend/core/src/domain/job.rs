use crate::id::{AudiobookId, JobId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Outline,
    Chapters,
    /// Parent TTS job that fans out one `TtsChapter` child per chapter.
    Tts,
    /// Single-chapter TTS unit (the real worker).
    TtsChapter,
    PostProcess,
    Cover,
    /// System-origin garbage collection.
    Gc,
    /// Translate every chapter from the audiobook's primary language to the
    /// job's `language` target.
    Translate,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Outline => "outline",
            JobKind::Chapters => "chapters",
            JobKind::Tts => "tts",
            JobKind::TtsChapter => "tts_chapter",
            JobKind::PostProcess => "post_process",
            JobKind::Cover => "cover",
            JobKind::Gc => "gc",
            JobKind::Translate => "translate",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "outline" => JobKind::Outline,
            "chapters" => JobKind::Chapters,
            "tts" => JobKind::Tts,
            "tts_chapter" => JobKind::TtsChapter,
            "post_process" => JobKind::PostProcess,
            "cover" => JobKind::Cover,
            "gc" => JobKind::Gc,
            "translate" => JobKind::Translate,
            _ => return None,
        })
    }
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

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Throttled => "throttled",
            JobStatus::Dead => "dead",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => JobStatus::Queued,
            "running" => JobStatus::Running,
            "completed" => JobStatus::Completed,
            "failed" => JobStatus::Failed,
            "throttled" => JobStatus::Throttled,
            "dead" => JobStatus::Dead,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Dead
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub user: Option<UserId>,
    pub audiobook: Option<AudiobookId>,
    pub parent: Option<JobId>,
    pub chapter_number: Option<u32>,
    pub status: JobStatus,
    pub progress_pct: f32,
    pub attempts: u32,
    pub max_attempts: u32,
    pub last_error: Option<String>,
    pub worker_id: Option<String>,
    pub queued_at: DateTime<Utc>,
    pub not_before: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub payload: Option<serde_json::Value>,
}
