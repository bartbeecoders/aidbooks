//! Text-to-speech clients.
//!
//! A tiny `TtsClient` trait with two implementations:
//!   * `MockTts` — fabricates audible-at-low-level PCM scaled to the text
//!     length; always available, used when no x.ai key is configured.
//!   * `XaiRestTts` — OpenAI-compatible REST client against
//!     `https://api.x.ai/v1/audio/speech` (or any compatible vendor).
//!
//! The trait returns raw PCM i16 mono at the configured sample rate so
//! the downstream audio module doesn't need to know which provider
//! produced the bytes.

pub mod mock;
pub mod xai_rest;

use async_trait::async_trait;
use listenai_core::Result;

/// Raw PCM audio block returned by any `TtsClient`.
#[derive(Debug, Clone)]
pub struct PcmAudio {
    pub samples: Vec<i16>,
    pub sample_rate_hz: u32,
    /// Whether the audio came from the mock path. Surfaced for logging.
    pub mocked: bool,
}

impl PcmAudio {
    /// Convenience: duration of the audio in milliseconds. Used by mock
    /// tests and admin tooling; not currently referenced on the hot path.
    #[allow(dead_code)]
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate_hz == 0 {
            0
        } else {
            (self.samples.len() as u64 * 1000) / self.sample_rate_hz as u64
        }
    }
}

#[async_trait]
pub trait TtsClient: Send + Sync {
    /// Render `text` with `voice` in `language` (BCP-47, e.g. `"en"`, `"nl"`).
    /// `"auto"` is also accepted by some providers and lets the upstream
    /// detect the language from the text.
    async fn synthesize(&self, text: &str, voice: &str, language: &str) -> Result<PcmAudio>;
    fn is_mock(&self) -> bool;
}

/// Trait-object wrapper that can be cheaply cloned and shared in AppState.
pub type SharedTts = std::sync::Arc<dyn TtsClient>;

/// Factory: pick the right implementation based on whether `xai_api_key`
/// is set. Falls back to the mock on any construction error. Language is
/// passed per call (see `TtsClient::synthesize`), not at construction.
pub fn build(
    api_key: &str,
    tts_url: &str,
    sample_rate_hz: u32,
    timeout_secs: u64,
) -> SharedTts {
    if api_key.trim().is_empty() {
        return std::sync::Arc::new(mock::MockTts::new(sample_rate_hz));
    }
    match xai_rest::XaiRestTts::new(
        api_key.to_string(),
        tts_url.to_string(),
        sample_rate_hz,
        timeout_secs,
    ) {
        Ok(c) => std::sync::Arc::new(c),
        Err(e) => {
            tracing::warn!(error = %e, "tts: failed to build REST client; falling back to mock");
            std::sync::Arc::new(mock::MockTts::new(sample_rate_hz))
        }
    }
}
