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

    // --- OpenRouter (Phase 3) ---
    /// OpenRouter API key. Empty string = mock mode (useful for local dev
    /// and CI; never enable for any real user).
    pub openrouter_api_key: String,
    pub openrouter_base_url: String,
    pub openrouter_request_timeout_secs: u64,
    /// Default model id used when no LLM row is picked (falls back to a
    /// Haiku-class model so dev burns through cheap tokens first).
    pub openrouter_default_model: String,

    // --- x.ai voice (Phase 4) ---
    /// x.ai API key. Empty string = mock mode.
    pub xai_api_key: String,
    /// x.ai REST API root (chat completions + `/language-models` catalog).
    /// Defaults to `https://api.x.ai/v1`.
    pub xai_base_url: String,
    /// x.ai TTS endpoint. Defaults to `https://api.x.ai/v1/tts`.
    pub xai_tts_url: String,
    /// BCP-47 language code sent with each request. `"auto"` lets x.ai
    /// detect the language from the text.
    pub xai_tts_language: String,
    pub xai_default_voice: String,
    pub xai_sample_rate_hz: u32,
    pub xai_request_timeout_secs: u64,
    /// Price the TTS layer bills per 1k characters of input. Defaults to
    /// `0.0042` USD (= $4.20 per 1M chars, which matches xAI's published
    /// Grok-TTS pricing as of early 2026). Override via env when xAI
    /// changes prices or you switch to a different vendor.
    pub xai_tts_cost_per_1k_chars: f64,

    // --- YouTube publishing (Phase 8) ---
    /// OAuth 2.0 client id for the Google Cloud project that owns the
    /// `youtube.upload` scope. Empty = publishing disabled.
    pub youtube_client_id: String,
    pub youtube_client_secret: String,
    /// Must match an Authorized Redirect URI configured on the OAuth client.
    pub youtube_redirect_uri: String,
    /// Where the OAuth callback bounces the user back to after a successful
    /// connect. The frontend reads `?connected=youtube` here to flash a toast.
    pub youtube_post_connect_redirect: String,
    /// Path to the `ffmpeg` binary used to assemble the publish-time MP4.
    /// Empty = the publish handler refuses to run.
    pub ffmpeg_bin: String,

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
            openrouter_api_key: String::new(),
            openrouter_base_url: "https://openrouter.ai/api/v1".into(),
            openrouter_request_timeout_secs: 120,
            openrouter_default_model: "anthropic/claude-haiku-4.5".into(),
            xai_api_key: String::new(),
            xai_base_url: "https://api.x.ai/v1".into(),
            xai_tts_url: "https://api.x.ai/v1/tts".into(),
            xai_tts_language: "en".into(),
            xai_default_voice: "eve".into(),
            xai_sample_rate_hz: 24_000,
            xai_request_timeout_secs: 180,
            xai_tts_cost_per_1k_chars: 0.0042,
            youtube_client_id: String::new(),
            youtube_client_secret: String::new(),
            youtube_redirect_uri:
                "http://localhost:8787/integrations/youtube/oauth/callback".into(),
            youtube_post_connect_redirect: "http://localhost:5173/app/settings".into(),
            ffmpeg_bin: "ffmpeg".into(),
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
