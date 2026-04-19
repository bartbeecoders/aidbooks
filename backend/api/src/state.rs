use listenai_core::config::Config;
use listenai_db::Db;
use std::sync::Arc;

use crate::llm::LlmClient;

/// Cheap-to-clone handle shared with every Axum handler.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    db: Db,
    llm: LlmClient,
}

impl AppState {
    pub fn new(config: Config, db: Db, llm: LlmClient) -> Self {
        Self {
            inner: Arc::new(Inner { config, db, llm }),
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
}
