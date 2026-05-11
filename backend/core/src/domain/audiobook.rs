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

/// Narrative style overlay applied to outline + chapter generation.
/// Reshapes plot beats, vocabulary and pacing so the same topic can be
/// realised as a thriller drama, a child-friendly read-along, an
/// educational explainer, etc. `Natural` (or `None` on the audiobook)
/// keeps whatever the genre alone would produce.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NarrationStyle {
    Natural,
    Drama,
    Humor,
    Sketch,
    Erotic,
    ChildFriendly,
    Educational,
}

impl NarrationStyle {
    /// Single short instruction the chapter / outline prompt embeds verbatim.
    /// Returning a `&'static str` keeps prompt rendering allocation-free.
    pub fn prompt_hint(self) -> &'static str {
        match self {
            NarrationStyle::Natural => "",
            NarrationStyle::Drama => "Adopt a high-stakes dramatic tone. Heighten conflict, raise emotional stakes, lean into tense pauses and weighty silences.",
            NarrationStyle::Humor => "Lean into humor. Add light wit, playful asides, comic timing and the occasional punchy one-liner without breaking the throughline.",
            NarrationStyle::Sketch => "Treat this like a comedy sketch. Snappy beats, exaggerated characters, broad punchlines, escalating absurdity. Keep it short, scene-shaped and quotable.",
            NarrationStyle::Erotic => "Use a sensual, suggestive register for adult listeners. No explicit anatomical detail, no minors, no non-consent — favour atmosphere, anticipation and longing over graphic description.",
            NarrationStyle::ChildFriendly => "Write for ages 5–10. Simple sentences, gentle rhythm, kindness over conflict. No violence, no romance, no scary cliffhangers. Repetition and warmth are good.",
            NarrationStyle::Educational => "Explain like a patient teacher. Define jargon, give one concrete example per concept, and recap each beat in plain words before moving on.",
        }
    }

    /// Lowercase string form for prompt vars / DB persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            NarrationStyle::Natural => "natural",
            NarrationStyle::Drama => "drama",
            NarrationStyle::Humor => "humor",
            NarrationStyle::Sketch => "sketch",
            NarrationStyle::Erotic => "erotic",
            NarrationStyle::ChildFriendly => "child_friendly",
            NarrationStyle::Educational => "educational",
        }
    }

    /// Inverse of [`Self::as_str`]; tolerant of unknown values (returns
    /// `None`) so a future style added by a forward client doesn't 500.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "natural" => NarrationStyle::Natural,
            "drama" => NarrationStyle::Drama,
            "humor" | "humour" => NarrationStyle::Humor,
            "sketch" => NarrationStyle::Sketch,
            "erotic" => NarrationStyle::Erotic,
            "child_friendly" | "children" | "kids" => NarrationStyle::ChildFriendly,
            "educational" => NarrationStyle::Educational,
            _ => return None,
        })
    }
}

/// Additive emotional intensity dial. Multiple tags combine —
/// `[Intense, Dramatic]` is a stronger thriller than `[Intense]` alone.
/// Empty list = neutral delivery (the chapter writer's own default).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NarrationIntensity {
    Intense,
    Dramatic,
    Emotional,
    Expressive,
}

impl NarrationIntensity {
    pub fn as_str(self) -> &'static str {
        match self {
            NarrationIntensity::Intense => "intense",
            NarrationIntensity::Dramatic => "dramatic",
            NarrationIntensity::Emotional => "emotional",
            NarrationIntensity::Expressive => "expressive",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "intense" => NarrationIntensity::Intense,
            "dramatic" => NarrationIntensity::Dramatic,
            "emotional" => NarrationIntensity::Emotional,
            "expressive" => NarrationIntensity::Expressive,
            _ => return None,
        })
    }
}

/// Voice-cast preset the UI uses to seed `voice_roles` and to remember
/// the picker layout the user chose. The audio pipeline still keys off
/// the canonical `voice_roles` map (narrator + dialogue_male +
/// dialogue_female); presets never extend the role set on their own.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VoicePreset {
    /// Single voice for everything (the legacy default).
    SingleNarrator,
    /// Single male voice for everything.
    SingleMale,
    /// Single female voice for everything.
    SingleFemale,
    /// Male narrator + male dialogue voice.
    DuoMale,
    /// Female narrator + female dialogue voice.
    DuoFemale,
    /// Narrator + male dialogue + female dialogue (the existing 3-voice cast).
    Mixed,
}

impl VoicePreset {
    pub fn as_str(self) -> &'static str {
        match self {
            VoicePreset::SingleNarrator => "single_narrator",
            VoicePreset::SingleMale => "single_male",
            VoicePreset::SingleFemale => "single_female",
            VoicePreset::DuoMale => "duo_male",
            VoicePreset::DuoFemale => "duo_female",
            VoicePreset::Mixed => "mixed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "single_narrator" | "narrator" => VoicePreset::SingleNarrator,
            "single_male" => VoicePreset::SingleMale,
            "single_female" => VoicePreset::SingleFemale,
            "duo_male" => VoicePreset::DuoMale,
            "duo_female" => VoicePreset::DuoFemale,
            "mixed" => VoicePreset::Mixed,
            _ => return None,
        })
    }
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
    /// Optional narrative style overlay (drama, humor, child_friendly,
    /// …). When set, the outline + chapter prompts inject the matching
    /// hint so the prose is reshaped to fit. `None` = keep the
    /// genre-driven default.
    #[serde(default)]
    pub narration_style: Option<NarrationStyle>,
    /// Additive emotional intensity tags combined into the chapter
    /// prompt and used to bias the speech-tag palette. Empty = neutral.
    #[serde(default)]
    pub narration_intensity: Vec<NarrationIntensity>,
    /// UX hint for the voice-cast picker. The audio pipeline ignores
    /// this and reads `voice_roles` directly; the field exists so the
    /// detail page can re-render the same picker layout (single /
    /// duo / mixed) the user originally chose.
    #[serde(default)]
    pub voice_preset: Option<VoicePreset>,
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
