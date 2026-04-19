//! Layered configuration: defaults → optional `Config.toml` → env vars
//! prefixed `LISTENAI_`.

use crate::{Error, Result};
use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub log: String,
    pub log_format: LogFormat,
    pub database_path: PathBuf,
    pub storage_path: PathBuf,
    pub request_timeout_secs: u64,
    pub cors_allow_origins: Vec<String>,
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
            database_path: PathBuf::from("./storage/db"),
            storage_path: PathBuf::from("./storage/audio"),
            request_timeout_secs: 30,
            cors_allow_origins: vec![
                "http://localhost:5173".into(),
                "http://127.0.0.1:5173".into(),
            ],
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
}
