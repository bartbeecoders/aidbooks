//! Animation feature: turn a chapter (text + WAV + waveform peaks) into an
//! animated companion video that the YouTube publisher can mux in place of
//! the static cover.
//!
//! Layered like the rest of the API crate:
//!
//!   * [`spec`]   — Rust mirror of the JSON `SceneSpec` contract that the
//!     Node (Revideo) sidecar consumes on stdin.
//!   * [`planner`] — Builds a `SceneSpec` from a chapter row + on-disk
//!     WAV/cover paths. Phase A keeps this trivial (one big paragraph
//!     scene); Phase B replaces it with real per-paragraph timing.
//!
//! On-disk layout (under `Config.storage_path`):
//!
//! ```text
//! <storage>/<audiobook_id>/<language>/
//!     ch-<n>.wav                  (input — narration; produced in Phase 4)
//!     ch-<n>.waveform.json        (input — peaks for the audio reactor)
//!     ch-<n>.video.mp4            (output — this module's product)
//! ```
//!
//! The publisher (`jobs::publishers::animate`) drives the Node sidecar;
//! everything in this module is plain data + planning logic so it stays
//! easy to unit-test without spawning a renderer.

pub mod cache;
pub mod fast_path;
pub mod hwenc;
pub mod manim_sidecar;
pub mod planner;
pub mod segments;
pub mod sidecar;
pub mod spec;
pub mod timing;
