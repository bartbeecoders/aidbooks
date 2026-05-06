//! Long-running tool that subscribes to /ws/audiobook/:id and forwards
//! progress events to the MCP client as `notifications/progress` messages.
//!
//! The tool blocks until either: the audiobook reaches a terminal status
//! (all jobs done/failed/cancelled), the client cancels, or `max_seconds`
//! elapses. The final tool result is the last-seen state snapshot.

use super::{ProgressSink, Registry, ToolHandler};
use crate::http_client::ApiClient;
use crate::proto::{CallToolResult, Tool};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

pub fn register(reg: &mut Registry, client: Arc<ApiClient>, ws_url: String) {
    reg.insert(SubscribeProgress {
        client,
        ws_url,
        tool: Tool {
            name: "audiobook_subscribe_progress".into(),
            description: "Subscribe to progress events for one audiobook. Streams MCP progress notifications as the pipeline (chapters → narration → cover art → publish) advances. Blocks until all jobs reach a terminal state, the client cancels, or `max_seconds` elapses. Pass `_meta.progressToken` in the call to receive intermediate updates; otherwise only the final snapshot is returned.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["audiobook_id"],
                "properties": {
                    "audiobook_id": {"type": "string"},
                    "max_seconds": {"type": "integer", "minimum": 1, "maximum": 3600, "default": 600,
                        "description": "Hard cap on how long the tool blocks. Default 10 minutes."},
                    "_token": {"type": "string", "description": "Bearer token override."}
                }
            }),
        },
    });
}

pub struct SubscribeProgress {
    client: Arc<ApiClient>,
    ws_url: String,
    tool: Tool,
}

impl ToolHandler for SubscribeProgress {
    fn descriptor(&self) -> &Tool {
        &self.tool
    }

    async fn call(
        &self,
        args: Value,
        progress: Option<ProgressSink>,
    ) -> Result<CallToolResult, String> {
        let id = args
            .get("audiobook_id")
            .and_then(|v| v.as_str())
            .ok_or("audiobook_id required")?
            .to_string();
        let max_seconds = args
            .get("max_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(600);
        let token_arg = args
            .get("_token")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let token = self
            .client
            .resolve_token(token_arg.as_deref())
            .ok_or("no bearer token (set LISTENAI_TOKEN or pass _token)")?;

        // The api accepts ?access_token= for browser-style WS clients that
        // can't send headers. We use that to keep this transport-agnostic.
        let url = format!(
            "{}/ws/audiobook/{}?access_token={}",
            self.ws_url.trim_end_matches('/'),
            urlencode(&id),
            urlencode(&token)
        );

        let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| format!("ws connect: {e}"))?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(max_seconds);
        let mut last_snapshot: Option<Value> = None;
        let mut events_seen = 0u64;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let next = tokio::time::timeout(remaining, ws.next()).await;
            let msg = match next {
                Err(_) => break, // deadline
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(format!("ws recv: {e}")),
                Ok(Some(Ok(m))) => m,
            };
            match msg {
                Message::Text(text) => {
                    events_seen += 1;
                    let parsed: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if let Some(sink) = &progress {
                        let (pct, summary) = summarise(&parsed);
                        sink.send(pct, Some(100.0), Some(summary));
                    }
                    if is_snapshot(&parsed) {
                        last_snapshot = Some(parsed.clone());
                        if all_terminal(&parsed) && events_seen > 1 {
                            // Don't bail on the very first snapshot — that's
                            // the current state, which may already be 100%.
                            // Fall through and let the api decide if more
                            // events arrive.
                        }
                    }
                    if all_terminal_in(&parsed) {
                        break;
                    }
                }
                Message::Ping(b) => {
                    let _ = futures_util::SinkExt::send(&mut ws, Message::Pong(b)).await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }

        let _ = futures_util::SinkExt::close(&mut ws).await;

        Ok(CallToolResult::json(json!({
            "audiobook_id": id,
            "events_received": events_seen,
            "final_snapshot": last_snapshot,
        })))
    }
}

fn is_snapshot(v: &Value) -> bool {
    v.get("type")
        .and_then(|t| t.as_str())
        .map(|t| t.eq_ignore_ascii_case("snapshot") || t.eq_ignore_ascii_case("Snapshot"))
        .unwrap_or(false)
        || v.get("jobs").is_some()
}

fn all_terminal(v: &Value) -> bool {
    let jobs = match v.get("jobs").and_then(|j| j.as_array()) {
        Some(a) => a,
        None => return false,
    };
    if jobs.is_empty() {
        return false;
    }
    jobs.iter().all(|j| {
        j.get("status")
            .and_then(|s| s.as_str())
            .map(|s| matches!(s, "succeeded" | "failed" | "cancelled" | "dead"))
            .unwrap_or(false)
    })
}

fn all_terminal_in(v: &Value) -> bool {
    is_snapshot(v) && all_terminal(v)
}

fn summarise(v: &Value) -> (f64, String) {
    if is_snapshot(v) {
        if let Some(jobs) = v.get("jobs").and_then(|j| j.as_array()) {
            let total = jobs.len() as f64;
            if total > 0.0 {
                let sum: f64 = jobs
                    .iter()
                    .map(|j| {
                        j.get("progress_pct")
                            .and_then(|p| p.as_f64())
                            .unwrap_or(0.0)
                    })
                    .sum();
                return (sum / total, format!("{} job(s)", jobs.len()));
            }
        }
        return (0.0, "snapshot".into());
    }
    // Single-job event shape from ProgressHub: forward the pct field.
    let pct = v
        .get("progress_pct")
        .and_then(|p| p.as_f64())
        .or_else(|| v.get("progress").and_then(|p| p.as_f64()))
        .unwrap_or(0.0);
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .or_else(|| v.get("type").and_then(|t| t.as_str()))
        .unwrap_or("event");
    (pct, kind.into())
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}
