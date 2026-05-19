use std::env;

/// Process-wide config sourced from env vars. All fields have defaults
/// so a bare `cargo run` works against a default-bound mold server.
#[derive(Debug, Clone)]
pub struct Config {
    /// Bind address for the mold-service HTTP listener.
    pub bind: String,
    /// Bind port for the mold-service HTTP listener.
    pub port: u16,
    /// Optional API key. When set, every non-`/healthz` request must
    /// carry `X-Api-Key: <value>`.
    pub api_key: Option<String>,
    /// Base URL of the upstream `mold serve` instance this service
    /// proxies to (e.g. `http://127.0.0.1:7680`).
    pub upstream_url: String,
    /// Optional API key forwarded to mold serve via `X-Api-Key`.
    pub upstream_api_key: Option<String>,
    /// Max concurrent in-flight generate requests against mold. Mold is
    /// single-model-at-a-time on the GPU, so 1 is the right default.
    pub max_concurrency: usize,
    /// Per-generate HTTP timeout in seconds. A cold model load can take
    /// minutes; 300s leaves enough headroom for first-call latency.
    pub timeout_secs: u64,
    /// Per-pull HTTP timeout in seconds. Pulling a flagship model from
    /// scratch over a slow link can take many minutes — defaults to 1h.
    pub pull_timeout_secs: u64,
    /// How long to hold the in-process semaphore after an OOM is
    /// detected. Mold marks a worker degraded after 3 consecutive
    /// failures and refuses requests for 60s; sleeping 65s outlasts
    /// that window so the next request sees a fresh worker.
    pub oom_cooldown_secs: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = env::var("MOLD_SERVICE_BIND").unwrap_or_else(|_| "127.0.0.1".into());
        let port = env::var("MOLD_SERVICE_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(7681);
        let api_key = env::var("MOLD_SERVICE_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let upstream_url =
            env::var("MOLD_UPSTREAM_URL").unwrap_or_else(|_| "http://127.0.0.1:7680".into());
        let upstream_api_key = env::var("MOLD_UPSTREAM_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let max_concurrency = env::var("MOLD_MAX_CONCURRENCY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(1);
        let timeout_secs = env::var("MOLD_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(300);
        let pull_timeout_secs = env::var("MOLD_PULL_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(3600);
        let oom_cooldown_secs = env::var("MOLD_OOM_COOLDOWN_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(65);

        Ok(Self {
            bind,
            port,
            api_key,
            upstream_url,
            upstream_api_key,
            max_concurrency,
            timeout_secs,
            pull_timeout_secs,
            oom_cooldown_secs,
        })
    }

    pub fn addr(&self) -> String {
        format!("{}:{}", self.bind, self.port)
    }
}
