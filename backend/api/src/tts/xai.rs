//! Real x.ai Realtime TTS client. One WebSocket session per synthesize
//! call: we push a text conversation item, ask for an audio response, and
//! collect `response.audio.delta` frames until `response.done`.
//!
//! Docs: https://docs.x.ai/developers/model-capabilities/audio/voice-agent
//!
//! Untested against a live account — requires a real `XAI_API_KEY`. The
//! wire format follows the public spec (OpenAI-Realtime-compatible).

use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use listenai_core::{Error, Result};
use serde_json::{json, Value};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use tracing::{debug, warn};

use super::{PcmAudio, TtsClient};

pub struct XaiTts {
    api_key: String,
    url: String,
    sample_rate_hz: u32,
    timeout: Duration,
}

impl XaiTts {
    pub fn new(api_key: String, url: String, sample_rate_hz: u32, timeout_secs: u64) -> Self {
        Self {
            api_key,
            url,
            sample_rate_hz,
            timeout: Duration::from_secs(timeout_secs),
        }
    }
}

#[async_trait]
impl TtsClient for XaiTts {
    async fn synthesize(&self, text: &str, voice: &str) -> Result<PcmAudio> {
        let pcm = timeout(self.timeout, drive_session(self, text, voice))
            .await
            .map_err(|_| Error::Upstream("x.ai tts: timeout".into()))??;
        Ok(PcmAudio {
            samples: pcm,
            sample_rate_hz: self.sample_rate_hz,
            mocked: false,
        })
    }

    fn is_mock(&self) -> bool {
        false
    }
}

async fn drive_session(client: &XaiTts, text: &str, voice: &str) -> Result<Vec<i16>> {
    let mut req = client
        .url
        .clone()
        .into_client_request()
        .map_err(|e| Error::Upstream(format!("x.ai url: {e}")))?;
    let headers = req.headers_mut();
    headers.insert(
        "Authorization",
        format!("Bearer {}", client.api_key)
            .parse()
            .map_err(|e| Error::Upstream(format!("x.ai auth header: {e}")))?,
    );

    let (mut ws, _response) = connect_async(req)
        .await
        .map_err(|e| Error::Upstream(format!("x.ai connect: {e}")))?;

    // Configure the session: audio-only output, PCM at our target rate,
    // disable server VAD since we drive the whole turn ourselves.
    let session_update = json!({
        "type": "session.update",
        "session": {
            "voice": voice,
            "output_audio_format": "pcm",
            "output_audio_sample_rate": client.sample_rate_hz,
            "turn_detection": null,
            "modalities": ["audio"],
        }
    });
    ws.send(Message::text(session_update.to_string()))
        .await
        .map_err(|e| Error::Upstream(format!("x.ai send session.update: {e}")))?;

    // Push the chapter text as a user turn.
    let item = json!({
        "type": "conversation.item.create",
        "item": {
            "type": "message",
            "role": "user",
            "content": [ { "type": "input_text", "text": text } ]
        }
    });
    ws.send(Message::text(item.to_string()))
        .await
        .map_err(|e| Error::Upstream(format!("x.ai send item: {e}")))?;

    // Ask for an audio response.
    let response = json!({
        "type": "response.create",
        "response": { "modalities": ["audio"] }
    });
    ws.send(Message::text(response.to_string()))
        .await
        .map_err(|e| Error::Upstream(format!("x.ai send response.create: {e}")))?;

    // Collect audio.delta payloads until response.done (or error).
    let mut samples: Vec<i16> = Vec::new();
    while let Some(frame) = ws.next().await {
        let msg = frame.map_err(|e| Error::Upstream(format!("x.ai ws read: {e}")))?;
        match msg {
            Message::Text(txt) => {
                let v: Value = serde_json::from_str(&txt)
                    .map_err(|e| Error::Upstream(format!("x.ai json: {e}")))?;
                let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
                match ty {
                    "response.audio.delta" => {
                        if let Some(b64) = v.get("delta").and_then(Value::as_str) {
                            let bytes = B64
                                .decode(b64)
                                .map_err(|e| Error::Upstream(format!("x.ai b64: {e}")))?;
                            append_pcm_le_i16(&mut samples, &bytes);
                        }
                    }
                    "response.done" => break,
                    "error" => {
                        let code = v
                            .get("error")
                            .and_then(|e| e.get("code"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let msg = v
                            .get("error")
                            .and_then(|e| e.get("message"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        return Err(Error::Upstream(format!("x.ai error {code}: {msg}")));
                    }
                    other => debug!(kind = other, "x.ai event (ignored)"),
                }
            }
            Message::Binary(bin) => {
                // Some servers send raw binary frames; treat as PCM.
                append_pcm_le_i16(&mut samples, &bin);
            }
            Message::Close(_) => break,
            Message::Ping(payload) => {
                ws.send(Message::Pong(payload))
                    .await
                    .map_err(|e| Error::Upstream(format!("x.ai pong: {e}")))?;
            }
            _ => {}
        }
    }

    // Best-effort close.
    if let Err(e) = ws.send(Message::Close(None)).await {
        warn!(error = %e, "x.ai close");
    }
    Ok(samples)
}

fn append_pcm_le_i16(out: &mut Vec<i16>, bytes: &[u8]) {
    let mut i = 0;
    while i + 1 < bytes.len() {
        out.push(i16::from_le_bytes([bytes[i], bytes[i + 1]]));
        i += 2;
    }
}
