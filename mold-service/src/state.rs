use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    /// Caps concurrent generate calls. Mold is single-model-at-a-time
    /// on the GPU; without this a fan-out of chapter-art jobs piles up
    /// against the same worker and pushes it past mold's 3-strike
    /// degrade threshold.
    pub semaphore: Arc<Semaphore>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrency));
        Self {
            config: Arc::new(config),
            semaphore,
        }
    }
}
