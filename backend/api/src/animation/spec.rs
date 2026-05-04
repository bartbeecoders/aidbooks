//! `SceneSpec` — JSON contract between the Rust planner and the Node
//! (Revideo) sidecar. Stable enough that the renderer can be swapped
//! (Remotion, Manim) without touching the planner.
//!
//! Versioned via [`SceneSpec::VERSION`]; bump on any breaking change.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The currently-emitted spec version. Mirrored on the Node side as
/// `EXPECTED_VERSION` in `backend/render/src/cli.ts` — bump both at once.
pub const SCENE_SPEC_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneSpec {
    pub version: u32,
    pub chapter: ChapterMeta,
    pub audio: AudioRef,
    pub theme: Theme,
    pub background: Background,
    pub scenes: Vec<Scene>,
    pub captions: Option<Captions>,
    pub output: Output,
}

impl SceneSpec {
    pub fn new(
        chapter: ChapterMeta,
        audio: AudioRef,
        background: Background,
        output: Output,
    ) -> Self {
        Self {
            version: SCENE_SPEC_VERSION,
            chapter,
            audio,
            theme: Theme::default(),
            background,
            scenes: Vec::new(),
            captions: None,
            output,
        }
    }

    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Phase B will populate captions from the existing `subtitles.rs`
    /// SRT generator; kept as a builder for symmetry. Allowed dead in
    /// Phase A so the `Captions` type and its frontmatter don't bitrot.
    #[allow(dead_code)]
    pub fn with_captions(mut self, captions: Captions) -> Self {
        self.captions = Some(captions);
        self
    }

    pub fn push(mut self, scene: Scene) -> Self {
        self.scenes.push(scene);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterMeta {
    pub number: u32,
    pub title: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioRef {
    /// Absolute path to the chapter narration WAV.
    pub wav: PathBuf,
    /// Absolute path to the matching `ch-<n>.waveform.json` (peaks file).
    /// Optional — if missing, the renderer skips the waveform-reactive
    /// accent layer instead of failing.
    pub peaks: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    /// Preset id matched on the renderer side. Phase A ships only
    /// `library`; the Phase C scene library adds `parchment` + `minimal`.
    pub preset: String,
    /// Overrides — colours are CSS hex strings (`#RRGGBB`).
    pub primary: Option<String>,
    pub accent: Option<String>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            preset: "library".into(),
            primary: None,
            accent: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Background {
    /// Solid colour fill — used by the placeholder renderer when no
    /// cover image is on disk.
    Color { color: String },
    /// Single image bed (the audiobook cover); the renderer applies a
    /// Ken-Burns slow pan + theme tint when `kenburns` is true.
    Image {
        src: PathBuf,
        #[serde(default)]
        kenburns: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scene {
    /// Title card. Phase A renders this as the only scene if no
    /// per-paragraph data is available, so the user always sees the
    /// chapter title at minimum.
    Title {
        start_ms: u64,
        end_ms: u64,
        title: String,
        subtitle: Option<String>,
    },
    /// Narrated paragraph scene. Phase B fills these in from the
    /// chapter body; Phase A only emits Title + Outro.
    Paragraph {
        start_ms: u64,
        end_ms: u64,
        text: String,
        /// Optional path to a per-paragraph illustration tile.
        tile: Option<PathBuf>,
        /// Highlight strategy. `"karaoke"` = per-word reveal at a
        /// constant cadence; `"none"` = no reveal animation.
        #[serde(default = "default_highlight")]
        highlight: String,
        /// Phase G.6 — when set, this paragraph renders as a Manim
        /// diagram instead of prose. The publisher routes to the
        /// Manim sidecar; the value is one of the kinds enumerated
        /// in `paragraphs::ALLOWED_VISUAL_KINDS`. `None` = render
        /// as prose via the chosen base path (fast / Revideo).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        visual_kind: Option<String>,
        /// Template-specific parameters (free-form JSON, validated
        /// by the Manim template at draw time). Only meaningful
        /// when `visual_kind` is set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        visual_params: Option<serde_json::Value>,
        /// Phase H — bespoke Manim code. Only set when
        /// `visual_kind == "custom_manim"`; the publisher ships this
        /// to the sidecar's `raw_scene` path instead of resolving a
        /// template by name. `None` here for a `custom_manim` kind
        /// means the code-gen LLM hasn't run yet, in which case the
        /// publisher falls back to prose with a warn log.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        manim_code: Option<String>,
    },
    /// Outro card — book cover + author + a small CTA.
    Outro {
        start_ms: u64,
        end_ms: u64,
        title: String,
        subtitle: Option<String>,
    },
}

fn default_highlight() -> String {
    "karaoke".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Captions {
    pub src: PathBuf,
    /// `true` = burn-in, `false` = soft track (ffmpeg muxes `subtitles.srt`
    /// later in the YouTube publisher). Defaults to soft.
    #[serde(default)]
    pub burn_in: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// Absolute path the renderer must write the MP4 to. The Rust side
    /// owns the layout under `<storage>/<audiobook>/<language>/`.
    pub mp4: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Output {
    /// 1080p at the caller-supplied frame rate. Default in the publisher
    /// is 24 (set by [`Config::animate_fps`](listenai_core::Config));
    /// 30 stays available for callers that need it (the YouTube encode
    /// in `publishers::youtube::run_single` is happy at either rate).
    pub fn hd_1080(mp4: PathBuf, fps: u32) -> Self {
        Self {
            mp4,
            width: 1920,
            height: 1080,
            fps: fps.max(1),
        }
    }
}

// ---------------------------------------------------------------------------
// NDJSON progress events the Node sidecar emits on stdout. The publisher
// parses one of these per line; anything else (warnings, traces) goes to
// stderr and is logged.
// ---------------------------------------------------------------------------

/// Mirrors the `{type, ...}` shape emitted by `backend/render/src/cli.ts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RenderEvent {
    /// Renderer accepted the spec and started initialising. Useful as a
    /// "moved past validation" signal — there's no progress yet.
    Started,
    /// Frame-by-frame progress. `frame` and `total` are 1-based; the
    /// publisher converts to a 0..1 fraction.
    Frame { frame: u64, total: u64 },
    /// Final mux + encode pass.
    Encoding { pct: f32 },
    /// Successful completion. `duration_ms` lets the publisher
    /// validate against the chapter's WAV duration.
    Done {
        // Reported by the renderer for cross-checks; the publisher
        // currently relies on the file existing on disk + the WAV's
        // own duration. Phase B's duration tolerance check will read
        // these.
        #[allow(dead_code)]
        mp4: PathBuf,
        #[allow(dead_code)]
        duration_ms: u64,
    },
    /// Renderer-side fatal — the publisher converts this to
    /// `JobOutcome::Fatal` so we don't burn retries on a malformed spec.
    Error { message: String },
    /// Long-lived sidecar (`server.ts`) only: emitted once at boot and
    /// again between renders. The pool waits on this before sending the
    /// next spec. Never emitted by the one-shot CLI.
    Ready,
    /// Long-lived sidecar only: emitted just before exit when stdin
    /// closes. Lets the pool distinguish a clean shutdown from a
    /// crash.
    Bye,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let spec = SceneSpec::new(
            ChapterMeta {
                number: 1,
                title: "Hello".into(),
                duration_ms: 5_000,
            },
            AudioRef {
                wav: PathBuf::from("/tmp/ch-1.wav"),
                peaks: None,
            },
            Background::Color {
                color: "#0F172A".into(),
            },
            Output::hd_1080(PathBuf::from("/tmp/out.mp4"), 24),
        )
        .push(Scene::Title {
            start_ms: 0,
            end_ms: 4_000,
            title: "Chapter 1".into(),
            subtitle: Some("Hello".into()),
        });
        let encoded = serde_json::to_string(&spec).unwrap();
        let decoded: SceneSpec = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.version, SCENE_SPEC_VERSION);
        assert_eq!(decoded.scenes.len(), 1);
    }

    #[test]
    fn parses_progress_event() {
        let line = r#"{"type":"frame","frame":42,"total":150}"#;
        let evt: RenderEvent = serde_json::from_str(line).unwrap();
        assert!(matches!(
            evt,
            RenderEvent::Frame {
                frame: 42,
                total: 150
            }
        ));
    }
}
