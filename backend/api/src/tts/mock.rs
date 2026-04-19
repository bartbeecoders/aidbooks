//! Mock TTS — produces PCM samples whose length is roughly proportional
//! to the input text. Used for local dev and CI when no real x.ai key is
//! configured. The waveform is intentionally quiet (−30 dBFS sine) so the
//! file is audibly a tone rather than a glitch if someone plays it back.

use async_trait::async_trait;
use listenai_core::Result;

use super::{PcmAudio, TtsClient};

/// Typical English narration cadence at 1x speed, rounded for convenience.
const CHARS_PER_SECOND: f64 = 14.0;
/// Low-amplitude sine so the rendered WAV is an audible test tone.
const TONE_FREQ_HZ: f32 = 220.0;
const TONE_AMPLITUDE: f32 = 0.03;

pub struct MockTts {
    pub sample_rate_hz: u32,
}

impl MockTts {
    pub fn new(sample_rate_hz: u32) -> Self {
        Self { sample_rate_hz }
    }
}

#[async_trait]
impl TtsClient for MockTts {
    async fn synthesize(&self, text: &str, _voice: &str) -> Result<PcmAudio> {
        let chars = text.chars().count().max(1) as f64;
        let duration_secs = (chars / CHARS_PER_SECOND).max(0.4);
        let sample_count = (duration_secs * self.sample_rate_hz as f64).round() as usize;

        let mut samples = Vec::with_capacity(sample_count);
        for n in 0..sample_count {
            let t = n as f32 / self.sample_rate_hz as f32;
            let sine = (2.0 * std::f32::consts::PI * TONE_FREQ_HZ * t).sin();
            samples.push((sine * TONE_AMPLITUDE * i16::MAX as f32) as i16);
        }

        Ok(PcmAudio {
            samples,
            sample_rate_hz: self.sample_rate_hz,
            mocked: true,
        })
    }

    fn is_mock(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn silence_is_proportional_to_text() {
        let m = MockTts::new(24_000);
        let short = m.synthesize("hello", "eve").await.unwrap();
        let long = m.synthesize(&"hello ".repeat(50), "eve").await.unwrap();
        assert!(long.samples.len() > short.samples.len() * 5);
        assert!(short.duration_ms() >= 350);
    }

    #[tokio::test]
    async fn mock_flag_is_true() {
        let m = MockTts::new(24_000);
        let a = m.synthesize("hi", "eve").await.unwrap();
        assert!(a.mocked);
        assert_eq!(a.sample_rate_hz, 24_000);
    }
}
