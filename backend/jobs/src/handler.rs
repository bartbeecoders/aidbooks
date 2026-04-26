//! Job handler trait + registry.
//!
//! The `api` crate implements one handler per [`JobKind`] (each closes over
//! `AppState`) and registers it here. The worker runtime routes a leased
//! [`JobRow`] to its handler and maps the outcome to `mark_completed` /
//! `mark_failed`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use listenai_core::domain::JobKind;
use listenai_core::Result;

use crate::hub::ProgressHub;
use crate::repo::{JobRepo, JobRow};

/// Per-call context passed to handlers.
#[derive(Clone)]
pub struct JobContext {
    pub repo: JobRepo,
    pub hub: ProgressHub,
}

impl JobContext {
    pub fn new(repo: JobRepo, hub: ProgressHub) -> Self {
        Self { repo, hub }
    }

    /// Convenience: publish a progress event AND persist it on the row.
    pub async fn progress(&self, job: &JobRow, stage: &str, pct: f32) {
        // Best-effort writes — a progress event that fails shouldn't fail the
        // whole job. Log on error so stuck UIs are traceable.
        if let Err(e) = self.repo.set_progress(&job.id, pct).await {
            tracing::warn!(job_id = %job.id, error = %e, "progress DB write failed");
        }
        if let Some(book) = job.audiobook_id.as_deref() {
            self.hub
                .publish(book, crate::hub::ProgressEvent::progress(job, stage, pct))
                .await;
        }
    }
}

/// Terminal outcome handlers return. `Retry` leaves it to the runtime to
/// requeue with backoff (or dead-letter if `attempts >= max_attempts`).
#[derive(Debug)]
pub enum JobOutcome {
    Done,
    /// Non-recoverable: jump straight to `dead`, skipping any retries left.
    Fatal(String),
    /// Transient failure — runtime applies the normal backoff + cap logic.
    Retry(String),
}

#[async_trait]
pub trait JobHandler: Send + Sync + 'static {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome>;
}

#[derive(Clone, Default)]
pub struct JobHandlerRegistry {
    inner: Arc<HashMap<JobKind, Arc<dyn JobHandler>>>,
}

impl JobHandlerRegistry {
    pub fn builder() -> JobHandlerRegistryBuilder {
        JobHandlerRegistryBuilder::default()
    }

    pub fn get(&self, kind: JobKind) -> Option<Arc<dyn JobHandler>> {
        self.inner.get(&kind).cloned()
    }

    pub fn registered_kinds(&self) -> Vec<JobKind> {
        self.inner.keys().copied().collect()
    }
}

#[derive(Default)]
pub struct JobHandlerRegistryBuilder {
    map: HashMap<JobKind, Arc<dyn JobHandler>>,
}

impl JobHandlerRegistryBuilder {
    pub fn register<H: JobHandler>(mut self, kind: JobKind, handler: H) -> Self {
        self.map.insert(kind, Arc::new(handler));
        self
    }

    pub fn build(self) -> JobHandlerRegistry {
        JobHandlerRegistry {
            inner: Arc::new(self.map),
        }
    }
}
