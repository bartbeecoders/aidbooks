//! x.ai Text-to-Speech REST client.
//!
//! Why this and not the Realtime WebSocket: the Realtime endpoint is a
//! conversational voice agent — without strict instructions it interprets
//! every user message as a query and *replies*, so the audio drifts away
//! from the chapter text. The TTS endpoint is deterministic.
//!
//! Wire shape (per <https://docs.x.ai/developers/model-capabilities/audio/text-to-speech>):
//!     POST /v1/tts
//!     {
//!       "text":     "<text to read>",
//!       "voice_id": "<eve|ara|rex|sal|leo>",
//!       "language": "<en|auto|...>",
//!       "output_format": { "codec": "pcm", "sample_rate": 24000 }
//!     }
//! Response body: raw 16-bit signed little-endian mono PCM at the requested
//! sample rate. Max 15 000 characters per request.

use std::time::Duration;

use async_trait::async_trait;
use listenai_core::{Error, Result};
use reqwest::Client;
use serde_json::json;

use super::{PcmAudio, TtsClient};

pub struct XaiRestTts {
    inner: Client,
    api_key: String,
    url: String,
    sample_rate_hz: u32,
}

impl XaiRestTts {
    pub fn new(
        api_key: String,
        url: String,
        sample_rate_hz: u32,
        timeout_secs: u64,
    ) -> Result<Self> {
        let inner = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent(concat!("listenai-api/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Other(anyhow::anyhow!("build tts http client: {e}")))?;
        Ok(Self {
            inner,
            api_key,
            url,
            sample_rate_hz,
        })
    }
}

#[async_trait]
impl TtsClient for XaiRestTts {
    async fn synthesize(&self, text: &str, voice: &str, language: &str) -> Result<PcmAudio> {
        let body = json!({
            "text": text,
            "voice_id": voice,
            "language": language,
            "output_format": {
                "codec": "pcm",
                "sample_rate": self.sample_rate_hz,
            },
        });

        let res = self
            .inner
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("tts request: {e}")))?;

        let status = res.status();
        if !status.is_success() {
            let snippet = res.text().await.unwrap_or_default();
            let snippet = if snippet.len() > 500 {
                format!("{}…", &snippet[..500])
            } else {
                snippet
            };
            return Err(Error::Upstream(format!("tts http {status}: {snippet}")));
        }

        let bytes = res
            .bytes()
            .await
            .map_err(|e| Error::Upstream(format!("tts body read: {e}")))?;

        let samples = decode_pcm_le_i16(&bytes);
        if samples.is_empty() {
            return Err(Error::Upstream(
                "tts returned empty audio body — wrong output_format?".into(),
            ));
        }

        Ok(PcmAudio {
            samples,
            sample_rate_hz: self.sample_rate_hz,
            mocked: false,
        })
    }

    fn is_mock(&self) -> bool {
        false
    }
}

fn decode_pcm_le_i16(bytes: &[u8]) -> Vec<i16> {
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i + 1 < bytes.len() {
        out.push(i16::from_le_bytes([bytes[i], bytes[i + 1]]));
        i += 2;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::decode_pcm_le_i16;

    #[test]
    fn decodes_le_pairs() {
        let bytes = [0x01, 0x00, 0xFF, 0xFF];
        assert_eq!(decode_pcm_le_i16(&bytes), vec![1_i16, -1]);
    }

    #[test]
    fn drops_trailing_odd_byte() {
        let bytes = [0x01, 0x00, 0x02];
        assert_eq!(decode_pcm_le_i16(&bytes), vec![1_i16]);
    }
}
