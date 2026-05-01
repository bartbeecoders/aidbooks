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

impl AudiobookLength {
    /// Number of chapters in the outline for this length preset.
    pub fn chapter_count(self) -> u32 {
        match self {
            AudiobookLength::Short => 3,
            AudiobookLength::Medium => 6,
            AudiobookLength::Long => 12,
        }
    }

    /// Target words per chapter. The LLM is free to deviate a little.
    pub fn words_per_chapter(self) -> u32 {
        match self {
            AudiobookLength::Short => 500,
            AudiobookLength::Medium => 1200,
            AudiobookLength::Long => 2500,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudiobookStatus {
    Draft,
    OutlinePending,
    OutlineReady,
    ChaptersRunning,
    TextReady,
    AudioReady,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChapterStatus {
    Pending,
    Running,
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
    /// Relative path under `Config.storage_path` to the cover image, when one
    /// has been generated. Served via `GET /audiobook/:id/cover`.
    pub cover_path: Option<String>,
    /// BCP-47 language code, e.g. `"en"`, `"nl"`, `"de"`. Drives both LLM
    /// content generation and TTS narration.
    pub language: String,
    /// X.ai TTS speech-tag palette suggested by the outline LLM (e.g.
    /// `["[pause]", "<whisper>", "<soft>"]`). The chapter generator embeds
    /// these inline in `chapter.body_md`; the X.ai TTS endpoint consumes
    /// them directly from the text. Empty = no tags suggested.
    #[serde(default)]
    pub tags: Vec<String>,
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
    pub chapter_art_path: Option<String>,
    pub audio_path: Option<String>,
    pub duration_ms: Option<u64>,
    pub status: ChapterStatus,
}
