use crate::id::LlmId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    OpenRouter,
    /// xAI native API (Grok models). Uses the same OpenAI-compatible
    /// chat-completions wire shape as OpenRouter, just a different host.
    Xai,
    /// Any OpenAI-compatible host (OpenAI proper, LMStudio, Ollama, vLLM…).
    /// Each row carries its own `base_url` + (optional) `api_key`; there
    /// is no global config for this provider.
    Openai,
    /// Self-hosted [`mold`](https://github.com/utensils/mold) image-gen
    /// server (CUDA/Metal). Each row points at `mold serve` via `base_url`
    /// and stores the optional `MOLD_API_KEY` in `api_key`. `model_id`
    /// carries the mold model slug (e.g. `flux2-klein:q8`).
    Mold,
    /// [fal.ai](https://fal.ai/docs/documentation) hosted image-gen API.
    /// Each row stores the `FAL_KEY` in `api_key`; `base_url` is optional
    /// and defaults to `https://fal.run` (sync mode). `model_id` carries
    /// the fal model slug (e.g. `fal-ai/flux/dev`, `fal-ai/flux/schnell`).
    Fal,
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
    /// Cross-language prose rewriter. Falls back to `Chapter` when no row
    /// is tagged for it, so existing setups keep working without changes.
    Translate,
    /// Generates raw Manim Python code per paragraph for the STEM
    /// diagram path (Phase H). Decoupled from `Chapter` because users
    /// often want a code-specialized model (DeepSeek-Coder, Qwen-Coder,
    /// Sonnet) for this even when their prose model is something else.
    /// Falls back to `Chapter` when no row is tagged for it, so books
    /// generated before this role landed keep rendering.
    ManimCode,
    /// Splits chapter prose into role-tagged segments (`narrator`,
    /// `dialogue_male`, `dialogue_female`) for the multi-voice
    /// narration feature. Falls back to `Chapter` when no row is
    /// tagged for it.
    VoiceExtract,
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
    /// Per-megapixel price for image-generation models. Always `0.0` for
    /// text models — they're priced by `cost_*_per_1k`.
    #[serde(default)]
    pub cost_per_megapixel: f64,
    pub enabled: bool,
    pub default_for: Vec<LlmRole>,
    /// What this model is for (`text`, `image`, future: `audio`, …).
    /// `None` means unspecified — treated as `"text"` by the picker.
    #[serde(default)]
    pub function: Option<String>,
    /// BCP-47 codes the model handles well. Empty = any language.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Picker tiebreaker; lower wins. Default 100.
    #[serde(default)]
    pub priority: i32,
}
