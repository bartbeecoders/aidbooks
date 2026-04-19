use crate::id::{AudiobookId, ChapterId, UserId, VoiceId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AudiobookLength {
    Short,
    Medium,
    Long,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudiobookStatus {
    Draft,
    OutlineReady,
    TextReady,
    AudioReady,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChapterStatus {
    Pending,
    TextReady,
    AudioReady,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Audiobook {
    pub id: AudiobookId,
    pub owner: UserId,
    pub title: String,
    pub topic: String,
    pub genre: Option<String>,
    pub length: AudiobookLength,
    pub primary_voice: Option<VoiceId>,
    pub status: AudiobookStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Chapter {
    pub id: ChapterId,
    pub audiobook: AudiobookId,
    pub number: u32,
    pub title: String,
    pub synopsis: Option<String>,
    pub target_words: Option<u32>,
    pub body_md: Option<String>,
    pub audio_path: Option<String>,
    pub duration_ms: Option<u64>,
    pub status: ChapterStatus,
}
