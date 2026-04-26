use listenai_core::config::Config;
use listenai_db::Db;
use listenai_jobs::{JobRepo, ProgressHub};
use std::sync::Arc;

use crate::llm::LlmClient;
use crate::tts::SharedTts;

/// Cheap-to-clone handle shared with every Axum handler.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    db: Db,
    llm: LlmClient,
    tts: SharedTts,
    job_repo: JobRepo,
    hub: ProgressHub,
}

impl AppState {
    pub fn new(
        config: Config,
        db: Db,
        llm: LlmClient,
        tts: SharedTts,
        job_repo: JobRepo,
        hub: ProgressHub,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                db,
                llm,
                tts,
                job_repo,
                hub,
            }),
        }
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    pub fn llm(&self) -> &LlmClient {
        &self.inner.llm
    }

    pub fn tts(&self) -> &SharedTts {
        &self.inner.tts
    }

    pub fn jobs(&self) -> &JobRepo {
        &self.inner.job_repo
    }

    pub fn hub(&self) -> &ProgressHub {
        &self.inner.hub
    }
}
