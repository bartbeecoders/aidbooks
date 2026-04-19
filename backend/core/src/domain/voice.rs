use crate::id::VoiceId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VoiceGender {
    Female,
    Male,
    Neutral,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Voice {
    pub id: VoiceId,
    pub name: String,
    /// Upstream provider, e.g. `"xai"`.
    pub provider: String,
    /// Provider's own identifier for this voice (e.g. `"eve"`).
    pub provider_voice_id: String,
    pub gender: VoiceGender,
    pub accent: String,
    pub language: String,
    pub sample_url: Option<String>,
    pub enabled: bool,
    pub premium_only: bool,
}
