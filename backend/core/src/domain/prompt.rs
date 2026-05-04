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
    /// Cover-art generation (book + chapter covers).
    Cover,
    /// Per-paragraph illustration tile.
    ParagraphImage,
    /// Cross-language prose rewrite.
    Translate,
    /// Visual-paragraph scene-extract pass (text in, JSON out).
    SceneExtract,
    /// Phase G — STEM diagram classifier. Per-paragraph LLM pass that
    /// labels paragraphs with a `visual_kind` (function_plot,
    /// free_body, …) and template-specific `visual_params` so the
    /// Manim render path knows what to draw. Only fires for books
    /// where the effective `is_stem` is true.
    ParagraphVisual,
    /// Phase H — bespoke Manim code generator. Renders paragraphs the
    /// classifier marked `visual_kind = "custom_manim"`. Returns a JSON
    /// blob with a Scene-class body the sidecar AST-screens then
    /// `exec`s. Driven by `LlmRole::ManimCode`, which the user can
    /// point at a code-specialized model independent of the prose
    /// model.
    ManimCode,
    /// Multi-voice narration: per-chapter LLM pass that splits prose
    /// into role-tagged segments (`narrator`, `dialogue_male`,
    /// `dialogue_female`). Cached on `chapter.voice_segments`. Only
    /// runs when the audiobook has `multi_voice_enabled = true`.
    VoiceExtract,
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
