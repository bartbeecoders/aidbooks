//! Layered configuration: defaults → optional `Config.toml` → env vars
//! prefixed `LISTENAI_`.

use crate::{Error, Result};
use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // --- networking ---
    pub host: String,
    pub port: u16,
    pub log: String,
    pub log_format: LogFormat,
    pub request_timeout_secs: u64,
    pub cors_allow_origins: Vec<String>,

    // --- storage ---
    pub database_path: PathBuf,
    pub storage_path: PathBuf,

    // --- auth ---
    /// HS256 signing secret for access-token JWTs.
    pub jwt_secret: String,
    /// HMAC key for hashing refresh tokens at rest.
    pub password_pepper: String,
    pub access_token_ttl_secs: u64,
    pub refresh_token_ttl_secs: u64,

    // --- dev-only ---
    /// Seeds a throw-away admin user on startup. Must stay `false` in prod.
    pub dev_seed: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8787,
            log: "listenai=debug,tower_http=debug,info".into(),
            log_format: LogFormat::Pretty,
            request_timeout_secs: 30,
            cors_allow_origins: vec![
                "http://localhost:5173".into(),
                "http://127.0.0.1:5173".into(),
            ],
            database_path: PathBuf::from("./storage/db"),
            storage_path: PathBuf::from("./storage/audio"),
            // Dev-safe defaults; prod MUST override via env.
            jwt_secret: "listenai-dev-jwt-secret-change-in-production".into(),
            password_pepper: "listenai-dev-pepper-change-in-production".into(),
            access_token_ttl_secs: 15 * 60,
            refresh_token_ttl_secs: 30 * 24 * 60 * 60,
            dev_seed: false,
        }
    }
}

impl Config {
    /// Load configuration from defaults, optional `./Config.toml`, and
    /// `LISTENAI_*` environment variables (in that order, with env winning).
    pub fn load() -> Result<Self> {
        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file("Config.toml"))
            .merge(Env::prefixed("LISTENAI_").split("__"))
            .extract()
            .map_err(|e| Error::Config(e.to_string()))
    }

    pub fn access_token_ttl(&self) -> Duration {
        Duration::from_secs(self.access_token_ttl_secs)
    }

    pub fn refresh_token_ttl(&self) -> Duration {
        Duration::from_secs(self.refresh_token_ttl_secs)
    }
}
