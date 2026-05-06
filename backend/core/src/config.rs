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
    /// Wall-clock cap on every HTTP request, enforced by the global
    /// `tower_http::TimeoutLayer`. Has to be ≥ the longest legitimate
    /// upstream timeout below (`xai_request_timeout_secs`, currently
    /// 180), otherwise synchronous LLM/image/translate handlers get
    /// killed mid-flight with a 408 even though the upstream call would
    /// have returned in time.
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

    // --- animation (companion video, feature/animation) ---
    /// Path to the Node binary used to drive the Revideo sidecar.
    /// Defaults to `node`.
    pub animate_node_bin: String,
    /// Absolute or relative path to the renderer entry point
    /// (`backend/render/dist/cli.js`). Empty = animate jobs refuse to
    /// run, mirroring `ffmpeg_bin`.
    pub animate_renderer_cmd: String,
    /// Mock mode: skip the Node sidecar and write a 5-second black MP4.
    /// Mirrors `MockTts` and keeps CI/integration tests fast.
    pub animate_mock: bool,
    /// Frames-per-second for the rendered MP4. Default 24 — every visual
    /// is karaoke + Ken Burns + crossfade, none of it motion-critical, so
    /// 24 is ~20 % cheaper to render than 30 with no perceptual diff.
    /// Bump to 30 for premium tiers if needed.
    pub animate_fps: u32,
    /// Number of parallel `AnimateChapter` workers. `0` = auto-detect
    /// (`min(available_parallelism, 4)`). Higher values trade CPU for
    /// wall-clock; the renderer's RSS (~400 MB Chromium) multiplies
    /// linearly so budget memory accordingly.
    pub animate_concurrency: u32,
    /// Phase F.1c — when `true`, route renders through a single
    /// ffmpeg invocation per chapter (Ken Burns + ASS subtitles +
    /// audio mux) instead of the Revideo / Chromium sidecar. ~5–10×
    /// faster on paragraph-dominant chapters but visually simpler:
    /// no animated title underlines, no per-word karaoke, no
    /// paragraph tiles, hard cuts between scenes. Off by default
    /// until shaken out on real content. Flipping this invalidates
    /// the F.1e spec-hash cache.
    pub animate_fast_path: bool,
    /// Phase F.1f.1 — hardware encoder selection for the fast path.
    /// `auto` (default) probes `ffmpeg -encoders` + the host's DRI
    /// render nodes and `/dev/nvidiactl` and picks NVENC > VAAPI >
    /// QSV > libx264. Force a specific encoder with `nvenc`,
    /// `vaapi`, `qsv`. Force CPU with `none` / `software` / `cpu`.
    /// Unrecognised values fall back to software with a warning.
    /// Doesn't invalidate the F.1e cache (encoder choice is
    /// environment, not content).
    pub animate_hwenc: String,
    /// Phase F.1f.1 — DRI render node used for VAAPI / QSV. Defaults
    /// to `/dev/dri/renderD128`, which is the right value on
    /// VAAPI-only hosts. On hybrid systems (e.g. discrete NVIDIA +
    /// integrated Intel) the integrated GPU is often `renderD129`;
    /// override here so the auto-detect picks it instead.
    pub animate_vaapi_device: String,
    /// Phase G.6 — path to the Manim Python sidecar entry. Empty =
    /// the per-segment STEM render path is *available* (the publisher
    /// still routes diagram-eligible chapters through it) but
    /// diagram scenes fall back to prose rendering with a warn log.
    /// Set to e.g. `backend/manim/.venv/bin/listenai-manim-server`.
    pub animate_manim_cmd: String,
    /// Phase G.6 — Python interpreter to launch the Manim sidecar
    /// with. Defaults to `python` on PATH. The recommended value on
    /// Arch with conda activated is the venv's interpreter directly:
    /// `backend/manim/.venv/bin/python` so Manim's deps resolve.
    pub animate_manim_python_bin: String,
    /// Phase G.6 — `LD_PRELOAD` value applied to every Manim
    /// sidecar process. Required on Arch to force the system
    /// libfontconfig ahead of the older one bundled in the
    /// `manimpango` wheel; same workaround the smoke recipes use.
    /// Empty = no preload.
    pub animate_manim_ld_preload: String,

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
            request_timeout_secs: 180,
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
            youtube_redirect_uri: "http://localhost:8787/integrations/youtube/oauth/callback"
                .into(),
            youtube_post_connect_redirect: "http://localhost:5173/app/settings".into(),
            ffmpeg_bin: "ffmpeg".into(),
            animate_node_bin: "node".into(),
            animate_renderer_cmd: String::new(),
            animate_mock: false,
            animate_fps: 24,
            animate_concurrency: 0,
            animate_fast_path: false,
            animate_hwenc: "auto".into(),
            animate_vaapi_device: "/dev/dri/renderD128".into(),
            animate_manim_cmd: String::new(),
            animate_manim_python_bin: "python".into(),
            animate_manim_ld_preload: String::new(),
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
