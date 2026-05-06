//! In-memory broadcast hub for live progress events.
//!
//! One `broadcast::Sender` per audiobook id. WebSocket handlers call
//! [`ProgressHub::subscribe`] to get a `broadcast::Receiver` and stream
//! events; workers call [`ProgressHub::publish`] when they advance.
//!
//! A slow subscriber only drops its own messages (the channel is bounded),
//! never stalls the worker.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use listenai_core::id::AudiobookId;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};
use tracing::debug;
use utoipa::ToSchema;

use crate::repo::JobRow;

/// Broadcast buffer per audiobook. Big enough that a slow browser tab
/// reconnecting doesn't miss the tail, small enough that a stale tab
/// doesn't pin megabytes of progress events in memory.
const CHANNEL_CAPACITY: usize = 128;

/// Wire event streamed to clients over the WebSocket.
///
/// `#[serde(tag = "type")]` keeps the JSON shape ergonomic for the
/// TS client: `{ type: "progress", ... }`, `{ type: "completed", ... }`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    /// Sent once to every new subscriber so the UI can render a correct
    /// initial state without a REST poll. Contains the latest known status
    /// of every job for that audiobook.
    Snapshot {
        audiobook_id: String,
        jobs: Vec<JobSnapshot>,
        at: DateTime<Utc>,
    },
    /// A job transitioned (queued → running, running → running+pct).
    Progress {
        job_id: String,
        kind: String,
        audiobook_id: Option<String>,
        stage: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        chapter: Option<u32>,
        pct: f32,
        #[serde(skip_serializing_if = "Option::is_none")]
        eta_seconds: Option<u64>,
        at: DateTime<Utc>,
    },
    /// Terminal success.
    Completed {
        job_id: String,
        kind: String,
        audiobook_id: Option<String>,
        at: DateTime<Utc>,
    },
    /// Terminal failure (`Dead` or `Failed` past `max_attempts`).
    Failed {
        job_id: String,
        kind: String,
        audiobook_id: Option<String>,
        error: String,
        at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct JobSnapshot {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub progress_pct: f32,
    pub attempts: u32,
    pub chapter_number: Option<u32>,
    pub last_error: Option<String>,
}

impl ProgressEvent {
    pub fn progress(job: &JobRow, stage: &str, pct: f32) -> Self {
        Self::Progress {
            job_id: job.id.clone(),
            kind: job.kind.as_str().to_string(),
            audiobook_id: job.audiobook_id.clone(),
            stage: stage.to_string(),
            chapter: job.chapter_number,
            pct: pct.clamp(0.0, 1.0),
            eta_seconds: None,
            at: Utc::now(),
        }
    }

    pub fn completed(job: &JobRow) -> Self {
        Self::Completed {
            job_id: job.id.clone(),
            kind: job.kind.as_str().to_string(),
            audiobook_id: job.audiobook_id.clone(),
            at: Utc::now(),
        }
    }

    pub fn failed(job: &JobRow, error: &str) -> Self {
        Self::Failed {
            job_id: job.id.clone(),
            kind: job.kind.as_str().to_string(),
            audiobook_id: job.audiobook_id.clone(),
            error: error.to_string(),
            at: Utc::now(),
        }
    }
}

#[derive(Clone, Default)]
pub struct ProgressHub {
    inner: Arc<Mutex<HashMap<String, broadcast::Sender<ProgressEvent>>>>,
}

impl ProgressHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create the channel for `audiobook_id` and return a receiver.
    pub async fn subscribe(
        &self,
        audiobook_id: &AudiobookId,
    ) -> broadcast::Receiver<ProgressEvent> {
        let mut map = self.inner.lock().await;
        let tx = map
            .entry(audiobook_id.0.clone())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        tx.subscribe()
    }

    /// Publish an event to all current subscribers for that audiobook.
    /// No-op if nobody is listening, so workers never block on the hub.
    pub async fn publish(&self, audiobook_id: &str, event: ProgressEvent) {
        let tx = {
            let map = self.inner.lock().await;
            map.get(audiobook_id).cloned()
        };
        if let Some(tx) = tx {
            let _ = tx.send(event);
        } else {
            // Nobody subscribed yet — fine; progress lives in the DB too.
            debug!(audiobook_id, "no subscribers, event dropped");
        }
    }

    /// Drop the channel for an audiobook. Called when its last subscriber
    /// leaves; prevents long-lived `Sender` instances from pinning memory
    /// for books the user closed days ago.
    pub async fn gc(&self, audiobook_id: &str) {
        let mut map = self.inner.lock().await;
        if let Some(tx) = map.get(audiobook_id) {
            if tx.receiver_count() == 0 {
                map.remove(audiobook_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use listenai_core::id::AudiobookId;

    #[tokio::test]
    async fn subscribe_receives_published_event() {
        let hub = ProgressHub::new();
        let book = AudiobookId("book-1".into());
        let mut rx = hub.subscribe(&book).await;

        hub.publish(
            "book-1",
            ProgressEvent::Snapshot {
                audiobook_id: "book-1".into(),
                jobs: vec![],
                at: Utc::now(),
            },
        )
        .await;

        let got = rx.recv().await.expect("event");
        matches!(got, ProgressEvent::Snapshot { .. });
    }

    #[tokio::test]
    async fn gc_drops_channel_when_no_subscribers() {
        let hub = ProgressHub::new();
        let book = AudiobookId("book-2".into());
        {
            let _rx = hub.subscribe(&book).await;
        } // rx dropped here
        hub.gc("book-2").await;
        // Next publish is a no-op; no crash, no subscribers.
        hub.publish(
            "book-2",
            ProgressEvent::Completed {
                job_id: "j".into(),
                kind: "tts".into(),
                audiobook_id: Some("book-2".into()),
                at: Utc::now(),
            },
        )
        .await;
    }

    #[test]
    fn progress_event_serializes_with_type_tag() {
        let e = ProgressEvent::Progress {
            job_id: "j".into(),
            kind: "chapters".into(),
            audiobook_id: Some("b".into()),
            stage: "narrating".into(),
            chapter: Some(2),
            pct: 0.5,
            eta_seconds: None,
            at: Utc::now(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"type\":\"progress\""));
        assert!(s.contains("\"chapter\":2"));
        // `eta_seconds` is None and must be omitted, not serialised as null.
        assert!(!s.contains("eta_seconds"));
    }
}
