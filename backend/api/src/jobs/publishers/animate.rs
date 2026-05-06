//! `JobKind::Animate` (parent) + `JobKind::AnimateChapter` (worker).
//!
//! Phase A: end-to-end shell.
//!
//! Parent (`Animate`):
//!   1. Enumerate chapters in the requested language.
//!   2. Fan out one `AnimateChapter` child per chapter.
//!   3. Poll children and aggregate progress for the WebSocket hub.
//!
//! Child (`AnimateChapter`):
//!   1. Load chapter row + audio path / duration.
//!   2. Build a `SceneSpec` via `animation::planner::plan`.
//!   3. Spawn the Node (Revideo) sidecar — JSON spec on stdin, NDJSON
//!      progress on stdout, MP4 on disk. In `animate_mock=true` mode we
//!      shortcut to ffmpeg + a black frame so CI doesn't need Node.
//!   4. Validate the output's duration against the WAV ± 100 ms.
//!
//! Phases B–E swap the planner for paragraph-aware logic, the renderer
//! for a real scene library, and integrate with the YouTube publisher.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use listenai_core::domain::JobKind;
use listenai_core::id::{AudiobookId, JobId, UserId};
use listenai_core::{Error, Result};
use listenai_jobs::{
    handler::{JobContext, JobOutcome},
    repo::{EnqueueRequest, JobRow},
    JobHandler,
};
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::{mpsc, OnceCell};
use tracing::{info, warn};

use crate::animation::cache;
use crate::animation::fast_path;
use crate::animation::manim_sidecar::{ManimRendererPool, ManimSidecarCfg};
use crate::animation::planner::{self, ParagraphTile, PlanInput};
use crate::animation::segments;
use crate::animation::sidecar::{self, RendererPool, SidecarCfg};
use crate::animation::spec::RenderEvent;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Parent: fan-out + aggregate.
// ---------------------------------------------------------------------------

pub struct AnimateParentHandler(pub AppState);

#[derive(Debug, Deserialize)]
struct ChapterRefRow {
    number: i64,
}

#[async_trait]
impl JobHandler for AnimateParentHandler {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome> {
        let user_id = job
            .user_id
            .clone()
            .ok_or_else(|| Error::Database("animate: missing user".into()))?;
        let audiobook_id = job
            .audiobook_id
            .clone()
            .ok_or_else(|| Error::Database("animate: missing audiobook".into()))?;
        let language = match &job.language {
            Some(l) => l.clone(),
            None => primary_language(&self.0, &audiobook_id).await?,
        };

        ctx.progress(&job, "loading", 0.0).await;

        let rows: Vec<ChapterRefRow> = self
            .0
            .db()
            .inner()
            .query(format!(
                "SELECT number FROM chapter \
                 WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang \
                 ORDER BY number ASC"
            ))
            .bind(("lang", language.clone()))
            .await
            .map_err(|e| Error::Database(format!("animate fan-out load: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("animate fan-out load (decode): {e}")))?;
        if rows.is_empty() {
            return Ok(JobOutcome::Fatal("no chapters to animate".into()));
        }

        let total = rows.len();

        // Phase H+ — diagram pre-flight. Every chapter's paragraphs need
        // `visual_kind` (and `manim_code` for `custom_manim`) before the
        // children can render diagrams. The chapter_paragraphs job
        // populates these on the happy path, but books that skipped it
        // (pre-feature, classifier failed, STEM flag was flipped on
        // after split) reach `animate` with empty metadata and would
        // render text-only. Run classify + code-gen here for every
        // chapter that's missing the data so the rest of the publisher
        // doesn't have to know about it. Idempotent — chapters that are
        // fully classified are no-ops.
        if load_is_stem(&self.0, &audiobook_id).await {
            ctx.progress(&job, "classifying", 0.0).await;
            // Operate on the primary language: paragraph metadata
            // lives on the primary chapter row (translations share it).
            let primary = primary_language(&self.0, &audiobook_id).await?;
            let book_ctx = match load_book_for_classify(&self.0, &audiobook_id).await {
                Ok(b) => Some(b),
                Err(e) => {
                    warn!(
                        error = %e,
                        audiobook_id = %audiobook_id,
                        "animate parent: load book context for classify failed; \
                         skipping pre-flight"
                    );
                    None
                }
            };
            if let Some(book) = book_ctx {
                let user_obj = UserId(user_id.clone());
                for (i, ch) in rows.iter().enumerate() {
                    ensure_chapter_classified(
                        &self.0,
                        &user_obj,
                        &audiobook_id,
                        &primary,
                        ch.number as u32,
                        &book,
                    )
                    .await;
                    let done = (i + 1) as f32;
                    // First half of the parent progress bar covers
                    // classify; render fills the second half below.
                    let pct = ((done / total as f32) * 0.5).clamp(0.0, 0.5);
                    ctx.progress(&job, "classifying", pct).await;
                }
            }
        }

        ctx.progress(&job, "fan_out", 0.5).await;
        let parent_id = JobId(job.id.clone());
        // Forward the theme preset from the parent payload onto every
        // child so each child renders with the same theme without a
        // second DB round-trip.
        let payload_theme = job
            .payload
            .as_ref()
            .and_then(|p| p.get("theme"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        for ch in &rows {
            let mut req = EnqueueRequest::new(JobKind::AnimateChapter)
                .with_user(UserId(user_id.clone()))
                .with_audiobook(AudiobookId(audiobook_id.clone()))
                .with_parent(parent_id.clone())
                .with_chapter(ch.number as u32)
                .with_language(language.clone())
                // Render is CPU-bound and largely deterministic. One real
                // attempt + one retry is plenty; further retries usually
                // mean the spec or assets are wrong, not transient.
                .with_max_attempts(2);
            if let Some(t) = payload_theme.as_deref() {
                req = req.with_payload(serde_json::json!({ "theme": t }));
            }
            if let Err(e) = ctx.repo.enqueue(req).await {
                warn!(
                    audiobook = %audiobook_id,
                    chapter = ch.number,
                    error = %e,
                    "animate parent: enqueue child failed"
                );
                return Ok(JobOutcome::Retry(format!("child enqueue failed: {e}")));
            }
        }

        // Aggregate.
        loop {
            let children = ctx.repo.children(&job.id).await?;
            let done = children.iter().filter(|c| c.status.is_terminal()).count();
            let any_dead = children
                .iter()
                .any(|c| c.status == listenai_core::domain::JobStatus::Dead);
            // Render fills the second half of the bar (classify owned
            // 0.0–0.5). For non-STEM books that skipped classify, the
            // bar still progresses 0.5 → 1.0 — slightly less precise
            // than the old 0.0 → 1.0 but consistent with the STEM
            // path so the UI doesn't have two scales to render.
            let frac = (done as f32 / total as f32).clamp(0.0, 1.0);
            let pct = 0.5 + frac * 0.5;
            ctx.progress(&job, "rendering", pct).await;

            if done == total {
                if any_dead {
                    return Ok(JobOutcome::Fatal(
                        "one or more chapters failed to render".into(),
                    ));
                }
                info!(
                    audiobook = %audiobook_id,
                    chapters = total,
                    "animate parent: all chapters rendered"
                );
                return Ok(JobOutcome::Done);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Child: single-chapter render via Node sidecar.
// ---------------------------------------------------------------------------

/// `JobKind::AnimateChapter` worker handler. Owns a lazily-initialized
/// pool of long-lived `node dist/server.js` sidecars (one per slot, sized
/// to `Config::animate_concurrency`). The pool is built on first non-mock
/// render so mock-mode and the no-renderer-cmd error path both stay
/// zero-cost.
pub struct AnimateChapterHandler {
    state: AppState,
    pool: Arc<OnceCell<Arc<RendererPool>>>,
    /// Phase G.6 — lazily-initialised Manim sidecar pool. None
    /// until the first STEM diagram render needs it; built once
    /// from `Config::animate_manim_cmd` + `animate_manim_python_bin`
    /// + `animate_manim_ld_preload`. Stays cold for non-STEM books
    /// so non-STEM users don't pay the Python/Manim startup cost.
    manim_pool: Arc<OnceCell<Option<Arc<ManimRendererPool>>>>,
}

impl AnimateChapterHandler {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            pool: Arc::new(OnceCell::new()),
            manim_pool: Arc::new(OnceCell::new()),
        }
    }

    /// Resolve `Config::animate_renderer_cmd` to the long-lived
    /// sidecar entry path. Accepts either the new `dist/server.js`
    /// directly or a legacy `dist/cli.js` (auto-derives `server.js`
    /// from the same directory and logs a deprecation note). Empty =
    /// caller is expected to handle the error before calling here.
    fn resolve_sidecar_path(renderer_cmd: &str) -> PathBuf {
        let raw = PathBuf::from(renderer_cmd);
        let file_name = raw.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if file_name == "cli.js" {
            warn!(
                cmd = renderer_cmd,
                "animate_renderer_cmd points at cli.js (one-shot); deriving server.js \
                 from the same directory. Update LISTENAI_ANIMATE_RENDERER_CMD to point \
                 at dist/server.js to silence this warning."
            );
            if let Some(parent) = raw.parent() {
                return parent.join("server.js");
            }
        }
        raw
    }

    async fn pool(&self) -> std::result::Result<Arc<RendererPool>, sidecar::RenderFailure> {
        let cfg = self.state.config();
        let cap = if cfg.animate_concurrency == 0 {
            std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(2)
                .clamp(1, 4)
        } else {
            cfg.animate_concurrency as usize
        };
        let node_bin = cfg.animate_node_bin.clone();
        let sidecar_cmd = Self::resolve_sidecar_path(&cfg.animate_renderer_cmd);
        let pool = self
            .pool
            .get_or_init(|| async move {
                RendererPool::new(SidecarCfg::new(node_bin, sidecar_cmd), cap)
            })
            .await;
        Ok(pool.clone())
    }

    /// Resolve (and lazily build) the Manim sidecar pool. Returns
    /// `None` when `animate_manim_cmd` is empty — the publisher
    /// then falls back to prose rendering for any diagram scene
    /// (with a warn log) instead of failing the whole render.
    async fn manim_pool(&self) -> Option<Arc<ManimRendererPool>> {
        let cfg = self.state.config();
        if cfg.animate_manim_cmd.trim().is_empty() {
            return None;
        }
        let cap = if cfg.animate_concurrency == 0 {
            std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(2)
                .clamp(1, 4)
        } else {
            cfg.animate_concurrency as usize
        };
        let python_bin = cfg.animate_manim_python_bin.clone();
        let sidecar_cmd = PathBuf::from(cfg.animate_manim_cmd.clone());
        let ld_preload = cfg.animate_manim_ld_preload.clone();
        let pool_opt = self
            .manim_pool
            .get_or_init(|| async move {
                Some(ManimRendererPool::new(
                    ManimSidecarCfg::new(python_bin, sidecar_cmd, ld_preload),
                    cap,
                ))
            })
            .await;
        pool_opt.clone()
    }
}

#[derive(Debug, Deserialize)]
struct ChapterRow {
    title: String,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    body_md: Option<String>,
}

#[async_trait]
impl JobHandler for AnimateChapterHandler {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome> {
        let state = &self.state;
        let audiobook_id = job
            .audiobook_id
            .clone()
            .ok_or_else(|| Error::Database("animate_chapter: missing audiobook".into()))?;
        let chapter_number = job
            .chapter_number
            .ok_or_else(|| Error::Database("animate_chapter: missing chapter_number".into()))?;
        let language = match &job.language {
            Some(l) => l.clone(),
            None => primary_language(state, &audiobook_id).await?,
        };

        ctx.progress(&job, "loading", 0.0).await;

        let chapter = match load_chapter(state, &audiobook_id, &language, chapter_number).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                return Ok(JobOutcome::Fatal(format!(
                    "chapter {chapter_number} not found for {audiobook_id}/{language}"
                )))
            }
            Err(e) => return Ok(JobOutcome::Retry(e.to_string())),
        };
        let duration_ms = chapter.duration_ms.unwrap_or(0).max(0) as u64;
        if duration_ms == 0 {
            return Ok(JobOutcome::Fatal(
                "chapter has no audio duration — narrate first".into(),
            ));
        }

        // Canonicalize every path we hand to the renderer so the JSON
        // contract is "absolute paths only". The renderer runs with
        // cwd=backend/render (so its own `node_modules` resolves), and
        // its `pathToUrl()` calls `resolve(p)` against that cwd — a
        // relative `./storage/audio/...` from `Config.storage_path`
        // would be resolved against the wrong directory and 404 in
        // Chromium.
        let storage_rel = state.config().storage_path.clone();
        let storage = std::fs::canonicalize(&storage_rel).map_err(|e| {
            Error::Other(anyhow::anyhow!(
                "canonicalize storage_path {storage_rel:?}: {e}"
            ))
        })?;
        let lang_dir = storage.join(&audiobook_id).join(&language);
        let wav_path = lang_dir.join(format!("ch-{chapter_number}.wav"));
        if !wav_path.exists() {
            return Ok(JobOutcome::Fatal(format!(
                "wav missing: {}",
                wav_path.display()
            )));
        }
        let waveform_path = {
            let p = lang_dir.join(format!("ch-{chapter_number}.waveform.json"));
            p.exists().then_some(p)
        };
        let cover_path = find_cover(state, &audiobook_id).and_then(|p| {
            // Cover lookup builds from the same (relative) storage path;
            // canonicalize for the renderer. Drop on error rather than
            // failing the whole render — falling back to a colour
            // background is a perfectly fine degradation.
            std::fs::canonicalize(&p).ok()
        });
        // Paragraph illustrations are anchored to the primary language
        // (translations share the same image set), so always load
        // tiles from the primary chapter row regardless of the
        // animate target language.
        let paragraph_tiles = load_paragraph_tiles(state, &audiobook_id, chapter_number).await;
        let output_mp4 =
            planner::output_mp4_path(&storage, &audiobook_id, &language, chapter_number);
        if let Some(parent) = output_mp4.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Ok(JobOutcome::Retry(format!(
                    "create output dir {}: {e}",
                    parent.display()
                )));
            }
        }

        let theme_preset = job
            .payload
            .as_ref()
            .and_then(|p| p.get("theme"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let cfg = state.config();
        let spec = planner::plan(PlanInput {
            audiobook_id: audiobook_id.clone(),
            language: language.clone(),
            chapter_number,
            chapter_title: chapter.title,
            chapter_body_md: chapter.body_md.unwrap_or_default(),
            audio_wav: wav_path.clone(),
            audio_duration_ms: duration_ms,
            waveform_json: waveform_path,
            cover_image: cover_path,
            paragraph_tiles,
            output_mp4: output_mp4.clone(),
            theme_preset,
            fps: cfg.animate_fps,
        });

        // Phase G.6 — STEM segment-mode is opt-in: requires fast
        // path on, the book's effective `is_stem` to be true, and
        // at least one paragraph scene with a `visual_kind`.
        // Otherwise we stay on whichever non-segment path the
        // existing flags select.
        let is_stem = load_is_stem(state, &audiobook_id).await;
        let has_diagrams = segments::has_diagram_scenes(&spec);
        let use_stem_segments =
            cfg.animate_fast_path && !cfg.animate_mock && is_stem && has_diagrams;
        // Surface the routing decision so users debugging "why
        // didn't Manim run?" can grep their logs and see which
        // condition flipped. Cheap (info-level, once per render).
        info!(
            audiobook = %audiobook_id,
            chapter = chapter_number,
            use_stem_segments,
            is_stem,
            has_diagrams,
            fast_path = cfg.animate_fast_path,
            mock = cfg.animate_mock,
            "animate_chapter: path selection"
        );

        // Pick the active render path. The label is folded into the
        // F.1e cache hash so flipping STEM, fast-path, or mock
        // invalidates cleanly.
        let render_path_label = if cfg.animate_mock {
            // Mock skips everything — single label is fine.
            cache::REVIDEO_PATH_LABEL
        } else if use_stem_segments {
            cache::FFMPEG_STEM_PATH_LABEL
        } else if cfg.animate_fast_path {
            cache::FFMPEG_PATH_LABEL
        } else {
            cache::REVIDEO_PATH_LABEL
        };

        // Skip-when-unchanged cache: if the prior render's hash file
        // matches what we'd compute now and the MP4 still exists on
        // disk, reuse it. Republishing a book where only metadata
        // changed (title, description) costs nothing this way.
        let cache_file = cache::cache_path(&output_mp4);
        let expected_hash = cache::compute_spec_hash(&spec, render_path_label);
        if output_mp4.exists() {
            if let Some(cached) = cache::read_cached_hash(&cache_file) {
                if cached == expected_hash {
                    info!(
                        audiobook = %audiobook_id,
                        chapter = chapter_number,
                        mp4 = %output_mp4.display(),
                        "animate_chapter: cache hit, skipping render"
                    );
                    ctx.progress(&job, "cached", 1.0).await;
                    return Ok(JobOutcome::Done);
                }
            }
        }

        ctx.progress(&job, "rendering", 0.05).await;
        let render_result = if cfg.animate_mock {
            run_mock_render(&output_mp4, duration_ms, &cfg.ffmpeg_bin).await
        } else if use_stem_segments {
            // Phase G.6 — per-segment STEM render. Manim handles
            // diagram paragraphs, fast-path handles the rest, ffmpeg
            // concats + muxes audio. Manim pool is `None` when the
            // sidecar isn't configured; segments::render_chapter
            // logs a warn and falls back to prose for diagrams in
            // that case.
            let manim_pool = self.manim_pool().await;
            if manim_pool.is_none() {
                warn!(
                    audiobook = %audiobook_id,
                    chapter = chapter_number,
                    "stem segments requested but animate_manim_cmd is empty; \
                     diagrams will fall back to prose rendering"
                );
            }
            // segments::render_chapter sends `SegmentProgress` so we
            // can surface "rendering diagram 3/12" instead of the bare
            // percentage. The label is what the WebSocket forwards as
            // `step` to the AnimationRow status line.
            let (tx, mut rx) = mpsc::unbounded_channel::<segments::SegmentProgress>();
            let progress_fut = async {
                while let Some(evt) = rx.recv().await {
                    let pct = 0.05 + 0.94 * evt.fraction.clamp(0.0, 1.0);
                    let step = format!(
                        "rendering {} {}/{}",
                        evt.kind.label(),
                        evt.index.saturating_add(1),
                        evt.total.max(1),
                    );
                    ctx.progress(&job, &step, pct).await;
                }
            };
            let render_fut = segments::render_chapter(
                &spec,
                &cfg.ffmpeg_bin,
                &cfg.animate_hwenc,
                &cfg.animate_vaapi_device,
                manim_pool,
                tx,
            );
            let (rendered, _) = tokio::join!(render_fut, progress_fut);
            rendered
        } else if cfg.animate_fast_path {
            // F.1c — single-shot ffmpeg per chapter. Drains a 0..1
            // fraction over the same channel idiom as the pool path
            // so the progress hub sees identical pacing.
            let (tx, mut rx) = mpsc::unbounded_channel::<f32>();
            let progress_fut = async {
                while let Some(frac) = rx.recv().await {
                    // Mirror the pool path's 0.05 → 0.99 envelope so
                    // the UI doesn't snap when the user flips paths.
                    let pct = 0.05 + 0.94 * frac.clamp(0.0, 1.0);
                    ctx.progress(&job, "rendering", pct).await;
                }
            };
            let render_fut = fast_path::render(
                &spec,
                &cfg.ffmpeg_bin,
                &cfg.animate_hwenc,
                &cfg.animate_vaapi_device,
                tx,
            );
            let (rendered, _) = tokio::join!(render_fut, progress_fut);
            rendered
        } else {
            if cfg.animate_renderer_cmd.trim().is_empty() {
                return Ok(JobOutcome::Fatal(
                    "animate_renderer_cmd is empty — set LISTENAI_ANIMATE_RENDERER_CMD or enable animate_mock".into(),
                ));
            }
            let pool = match self.pool().await {
                Ok(p) => p,
                Err(sidecar::RenderFailure::Transient(msg)) => return Ok(JobOutcome::Retry(msg)),
                Err(sidecar::RenderFailure::Fatal(msg)) => return Ok(JobOutcome::Fatal(msg)),
            };
            // Drain progress events from the pool through the
            // existing job-progress hub. Pool drops `tx` when the
            // render completes (success or failure), which terminates
            // the drain loop naturally.
            let (tx, mut rx) = mpsc::unbounded_channel::<RenderEvent>();
            let progress_fut = async {
                while let Some(evt) = rx.recv().await {
                    match evt {
                        RenderEvent::Started => ctx.progress(&job, "rendering", 0.10).await,
                        RenderEvent::Frame { frame, total } => {
                            let pct = if total == 0 {
                                0.10
                            } else {
                                // Reserve 0.10–0.85 for frame
                                // rendering, 0.85–0.99 for encoding.
                                0.10 + 0.75 * (frame as f32 / total as f32).clamp(0.0, 1.0)
                            };
                            ctx.progress(&job, "rendering", pct).await;
                        }
                        RenderEvent::Encoding { pct } => {
                            let bounded = 0.85 + 0.14 * pct.clamp(0.0, 1.0);
                            ctx.progress(&job, "encoding", bounded).await;
                        }
                        RenderEvent::Done { .. } => ctx.progress(&job, "encoded", 0.99).await,
                        // Ready / Bye / Error never escape the pool
                        // — render() handles them internally.
                        _ => {}
                    }
                }
            };
            let render_fut = pool.render(&spec, tx);
            let (rendered, _) = tokio::join!(render_fut, progress_fut);
            rendered
        };

        match render_result {
            Ok(()) => {}
            Err(sidecar::RenderFailure::Transient(msg)) => return Ok(JobOutcome::Retry(msg)),
            Err(sidecar::RenderFailure::Fatal(msg)) => return Ok(JobOutcome::Fatal(msg)),
        }

        if !output_mp4.exists() {
            return Ok(JobOutcome::Retry(format!(
                "renderer reported success but {} is missing",
                output_mp4.display()
            )));
        }

        // Persist the spec hash next to the MP4. A failed write means
        // the next run misses the cache and re-renders — strictly less
        // good than catching the cache, but never wrong. Don't fail the
        // job over it.
        if let Err(e) = cache::write_hash(&cache_file, &expected_hash) {
            warn!(
                audiobook = %audiobook_id,
                chapter = chapter_number,
                cache_file = %cache_file.display(),
                error = %e,
                "animate_chapter: cache write failed"
            );
        }

        info!(
            audiobook = %audiobook_id,
            chapter = chapter_number,
            mp4 = %output_mp4.display(),
            "animate_chapter: rendered"
        );
        ctx.progress(&job, "ready", 1.0).await;
        Ok(JobOutcome::Done)
    }
}

// ---------------------------------------------------------------------------
// Renderer drivers.
// ---------------------------------------------------------------------------
//
// Production renders go through `RendererPool` (animation::sidecar) —
// the per-call `node dist/cli.js` spawn was removed in F.1a. Only the
// mock fallback lives here; it stays inline because it doesn't need
// the pool's lifecycle machinery.

/// Mock renderer: produces a black 1080p mp4 of the requested duration
/// using ffmpeg's `lavfi` color source. Mirrors `MockTts` — keeps the
/// happy path testable without Node + Revideo on the host.
async fn run_mock_render(
    output_mp4: &Path,
    duration_ms: u64,
    ffmpeg_bin: &str,
) -> std::result::Result<(), sidecar::RenderFailure> {
    let bin = if ffmpeg_bin.trim().is_empty() {
        "ffmpeg"
    } else {
        ffmpeg_bin
    };
    let secs = (duration_ms as f64 / 1000.0).max(0.5);
    let mut cmd = Command::new(bin);
    cmd.arg("-y")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(format!("color=c=black:s=1920x1080:r=30:d={secs:.3}"))
        .arg("-c:v")
        .arg("libx264")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-preset")
        .arg("veryfast")
        .arg("-an")
        .arg(output_mp4)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = cmd
        .output()
        .await
        .map_err(|e| sidecar::RenderFailure::Transient(format!("spawn ffmpeg (mock): {e}")))?;
    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(sidecar::RenderFailure::Transient(format!(
            "ffmpeg (mock) exited with {}: {}",
            output.status,
            tail.trim_end()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

async fn primary_language(state: &AppState, audiobook_id: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        language: Option<String>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!("SELECT language FROM audiobook:`{audiobook_id}`"))
        .await
        .map_err(|e| Error::Database(format!("animate primary_language: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("animate primary_language (decode): {e}")))?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.language)
        .unwrap_or_else(|| "en".to_string()))
}

/// Effective STEM flag for an audiobook: `stem_override > stem_detected
/// > false`. Same fallback the detail endpoint uses. Returns `false`
/// on any DB error so a transient query hiccup degrades to "treat as
/// non-STEM" (i.e. take the existing render path) rather than failing
/// the render outright.
async fn load_is_stem(state: &AppState, audiobook_id: &str) -> bool {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        stem_detected: Option<bool>,
        #[serde(default)]
        stem_override: Option<bool>,
    }
    let rows: Vec<Row> = match state
        .db()
        .inner()
        .query(format!(
            "SELECT stem_detected, stem_override FROM audiobook:`{audiobook_id}`"
        ))
        .await
    {
        Ok(mut r) => match r.take(0) {
            Ok(rs) => rs,
            Err(e) => {
                warn!(
                    error = %e,
                    audiobook_id,
                    "animate: decode stem flags failed; treating as non-STEM"
                );
                return false;
            }
        },
        Err(e) => {
            warn!(
                error = %e,
                audiobook_id,
                "animate: load stem flags failed; treating as non-STEM"
            );
            return false;
        }
    };
    rows.into_iter()
        .next()
        .map(|r| {
            r.stem_override
                .unwrap_or_else(|| r.stem_detected.unwrap_or(false))
        })
        .unwrap_or(false)
}

async fn load_chapter(
    state: &AppState,
    audiobook_id: &str,
    language: &str,
    chapter_number: u32,
) -> Result<Option<ChapterRow>> {
    let rows: Vec<ChapterRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT title, duration_ms, body_md FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", chapter_number as i64))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("animate load chapter: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("animate load chapter (decode): {e}")))?;
    Ok(rows.into_iter().next())
}

fn find_cover(state: &AppState, audiobook_id: &str) -> Option<PathBuf> {
    let dir = state.config().storage_path.join(audiobook_id);
    for ext in ["png", "jpg", "jpeg", "webp"] {
        let p = dir.join(format!("cover.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Load paragraph illustration tiles for the planner.
///
/// Paragraph images are anchored to the audiobook's *primary* language
/// (translations share the same image set), so we always fetch them
/// from the primary chapter row regardless of which language we're
/// animating. Each `chapter.paragraphs[i]` may have multiple
/// `image_paths` (different ordinals); we take the first non-empty
/// entry — Phase F can revisit if we want to crossfade between them.
///
/// Errors degrade silently to an empty vec — a missing tile is not
/// fatal, the planner will emit text-only scenes for those paragraphs.
async fn load_paragraph_tiles(
    state: &AppState,
    audiobook_id: &str,
    chapter_number: u32,
) -> Vec<ParagraphTile> {
    let primary = match primary_language(state, audiobook_id).await {
        Ok(l) => l,
        Err(e) => {
            warn!(error = %e, audiobook_id, "animate: load primary lang failed");
            return Vec::new();
        }
    };

    #[derive(Deserialize)]
    struct DbRow {
        #[serde(default)]
        paragraphs: Option<Vec<DbParagraph>>,
    }
    #[derive(Deserialize)]
    struct DbParagraph {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        image_paths: Vec<String>,
        // Phase G — diagram label from the per-paragraph visual
        // classifier (G.2). Optional: only set on STEM books.
        #[serde(default)]
        visual_kind: Option<String>,
        #[serde(default)]
        visual_params: Option<serde_json::Value>,
        // Phase H — bespoke Manim code from the ManimCode LLM.
        // Only populated when visual_kind == "custom_manim".
        #[serde(default)]
        manim_code: Option<String>,
    }

    let mut resp = match state
        .db()
        .inner()
        .query(format!(
            "SELECT paragraphs FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", chapter_number as i64))
        .bind(("lang", primary))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, audiobook_id, chapter_number, "animate: load paragraphs failed");
            return Vec::new();
        }
    };
    let rows: Vec<DbRow> = match resp.take(0) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, audiobook_id, chapter_number, "animate: decode paragraphs failed");
            return Vec::new();
        }
    };

    let paragraphs = rows
        .into_iter()
        .next()
        .and_then(|r| r.paragraphs)
        .unwrap_or_default();
    let storage = &state.config().storage_path;

    paragraphs
        .into_iter()
        .filter_map(|p| {
            let text = p.text.filter(|s| !s.trim().is_empty())?;

            // Canonicalize the tile path, if any. Missing files drop
            // to None rather than killing the row — a paragraph might
            // legitimately have a visual_kind without a tile (STEM
            // diagram-only paragraph).
            let image_path = p
                .image_paths
                .into_iter()
                .find(|s| !s.is_empty())
                .and_then(|rel| std::fs::canonicalize(storage.join(rel)).ok());

            let visual_kind = p
                .visual_kind
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            // Drop rows that have neither a tile nor a diagram label —
            // they'd be matched by the planner but contribute nothing.
            if image_path.is_none() && visual_kind.is_none() {
                return None;
            }

            let manim_code = p
                .manim_code
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            Some(ParagraphTile {
                text,
                image_path,
                visual_kind,
                visual_params: p.visual_params,
                manim_code,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Diagram pre-flight (classify + manim code-gen) for the parent handler.
// ---------------------------------------------------------------------------

/// Audiobook fields the classifier + code-gen prompts need. Loaded once
/// at the top of the parent and threaded through each per-chapter call.
struct BookCtx {
    title: String,
    topic: String,
    genre: Option<String>,
}

async fn load_book_for_classify(state: &AppState, audiobook_id: &str) -> Result<BookCtx> {
    #[derive(Deserialize)]
    struct Row {
        title: String,
        topic: String,
        #[serde(default)]
        genre: Option<String>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT title, topic, genre FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("animate classify load book: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("animate classify decode book: {e}")))?;
    let row = rows.into_iter().next().ok_or_else(|| {
        Error::Database(format!("animate classify: book {audiobook_id} not found"))
    })?;
    Ok(BookCtx {
        title: row.title,
        topic: row.topic,
        genre: row.genre,
    })
}

/// One paragraph as it currently lives on the chapter row. Mirrors the
/// fields the classifier output cares about so we can decide what work
/// (classify vs. code-gen) is still outstanding.
#[derive(Debug, Deserialize)]
struct ExistingParagraph {
    #[serde(default)]
    index: i64,
    #[serde(default)]
    text: String,
    #[serde(default)]
    char_count: Option<i64>,
    #[serde(default)]
    scene_description: Option<String>,
    #[serde(default)]
    image_paths: Vec<String>,
    #[serde(default)]
    visual_kind: Option<String>,
    #[serde(default)]
    visual_params: Option<serde_json::Value>,
    #[serde(default)]
    manim_code: Option<String>,
}

/// Idempotent: bring one chapter's paragraph metadata up to the level
/// the renderer expects. Three branches by current state:
///   * paragraphs absent entirely → split + scenes + visuals + code-gen
///   * paragraphs exist but no `visual_kind` on any → classify pass
///     (preserve scenes/images), then code-gen for any new
///     `custom_manim`
///   * paragraphs + `visual_kind` already, but a `custom_manim` is
///     missing `manim_code` → code-gen for just those, preserving
///     everything else
///
/// All three paths degrade gracefully on LLM failure: the corresponding
/// HashMap stays empty and `merge_for_persist` (or the ad-hoc merge
/// for the partial branches) writes back what we *do* have so we
/// never wipe working data.
async fn ensure_chapter_classified(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    primary_language: &str,
    chapter_number: u32,
    book: &BookCtx,
) {
    #[derive(Deserialize)]
    struct ChapterRow {
        id: surrealdb::sql::Thing,
        title: String,
        #[serde(default)]
        body_md: Option<String>,
        #[serde(default)]
        paragraphs: Option<Vec<ExistingParagraph>>,
    }
    let chapter: ChapterRow = match state
        .db()
        .inner()
        .query(format!(
            "SELECT id, title, body_md, paragraphs FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", chapter_number as i64))
        .bind(("lang", primary_language.to_string()))
        .await
    {
        Ok(mut resp) => match resp.take::<Vec<ChapterRow>>(0) {
            Ok(mut rows) => match rows.pop() {
                Some(c) => c,
                None => {
                    warn!(
                        audiobook = %audiobook_id,
                        chapter = chapter_number,
                        "animate classify: chapter row not found in primary language; skipping"
                    );
                    return;
                }
            },
            Err(e) => {
                warn!(
                    error = %e,
                    audiobook = %audiobook_id,
                    chapter = chapter_number,
                    "animate classify: decode chapter failed; skipping"
                );
                return;
            }
        },
        Err(e) => {
            warn!(
                error = %e,
                audiobook = %audiobook_id,
                chapter = chapter_number,
                "animate classify: load chapter failed; skipping"
            );
            return;
        }
    };

    let chapter_id = chapter.id.id.to_raw();
    let body = chapter.body_md.as_deref().unwrap_or("");
    if body.trim().is_empty() {
        // Nothing to classify against — the chapter wasn't generated
        // properly. Render path will fall back to text-only.
        return;
    }
    let existing = chapter.paragraphs.unwrap_or_default();

    // ----- Branch A: full pipeline (no paragraphs at all) -------------
    if existing.is_empty() {
        let paragraphs = crate::generation::paragraphs::split(body);
        if paragraphs.is_empty() {
            return;
        }
        let scenes = crate::generation::paragraphs::extract_scenes(
            state,
            user,
            audiobook_id,
            &book.title,
            &book.topic,
            book.genre.as_deref(),
            &chapter.title,
            &paragraphs,
        )
        .await;
        let visuals = crate::generation::paragraphs::extract_visual_kinds(
            state,
            user,
            audiobook_id,
            &book.title,
            &book.topic,
            book.genre.as_deref(),
            &chapter.title,
            &paragraphs,
        )
        .await;
        let manim_codes = run_code_gen_for_custom_manim(
            state,
            user,
            audiobook_id,
            book,
            &chapter.title,
            &paragraphs,
            &visuals,
        )
        .await;
        let merged = crate::generation::paragraphs::merge_for_persist(
            &paragraphs,
            &scenes,
            &visuals,
            &manim_codes,
        );
        if let Err(e) = crate::generation::paragraphs::persist(state, &chapter_id, merged).await {
            warn!(
                error = %e,
                audiobook = %audiobook_id,
                chapter = chapter_number,
                "animate classify: persist (full) failed"
            );
        }
        return;
    }

    // ----- Branch B/C: partial. Existing paragraphs present. ----------
    let none_classified = existing.iter().all(|p| {
        p.visual_kind
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
    });
    let custom_missing_code: Vec<u32> = existing
        .iter()
        .filter(|p| p.visual_kind.as_deref() == Some("custom_manim"))
        .filter(|p| {
            p.manim_code
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        })
        .map(|p| p.index.max(0) as u32)
        .collect();

    if !none_classified && custom_missing_code.is_empty() {
        // Already fully populated. No-op.
        return;
    }

    // The classifier wants `Paragraph` structs (index, text, char_count).
    let classifier_input: Vec<crate::generation::paragraphs::Paragraph> = existing
        .iter()
        .filter(|p| !p.text.trim().is_empty())
        .map(|p| crate::generation::paragraphs::Paragraph {
            index: p.index.max(0) as u32,
            text: p.text.clone(),
            char_count: p
                .char_count
                .unwrap_or_else(|| p.text.chars().count() as i64)
                .max(0) as u32,
        })
        .collect();

    // Run classify only when nothing is currently labelled. Re-running
    // a partial classification would risk overwriting good labels with
    // nothing if the LLM degrades on this attempt.
    let visuals = if none_classified {
        crate::generation::paragraphs::extract_visual_kinds(
            state,
            user,
            audiobook_id,
            &book.title,
            &book.topic,
            book.genre.as_deref(),
            &chapter.title,
            &classifier_input,
        )
        .await
    } else {
        std::collections::HashMap::new()
    };

    // Determine which paragraphs need code-gen this run: union of
    // newly-classified `custom_manim` rows and any pre-existing
    // `custom_manim` rows that are still missing code.
    let mut code_input_paragraphs: Vec<crate::generation::paragraphs::Paragraph> = Vec::new();
    let mut wanted: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for (idx, v) in &visuals {
        if v.visual_kind == "custom_manim" {
            wanted.insert(*idx);
        }
    }
    for idx in &custom_missing_code {
        wanted.insert(*idx);
    }
    if !wanted.is_empty() {
        for p in &classifier_input {
            if wanted.contains(&p.index) {
                code_input_paragraphs.push(p.clone());
            }
        }
    }
    let mut new_visuals_kinds: std::collections::HashMap<u32, String> =
        std::collections::HashMap::new();
    for (idx, v) in &visuals {
        new_visuals_kinds.insert(*idx, v.visual_kind.clone());
    }
    // Pre-existing custom_manim rows count as "custom_manim" for the
    // purposes of code-gen routing too.
    for p in &existing {
        if p.visual_kind.as_deref() == Some("custom_manim") {
            new_visuals_kinds
                .entry(p.index.max(0) as u32)
                .or_insert_with(|| "custom_manim".into());
        }
    }

    let manim_codes: std::collections::HashMap<u32, String> = if code_input_paragraphs.is_empty() {
        std::collections::HashMap::new()
    } else {
        let custom = crate::generation::manim_code::custom_paragraphs(
            &code_input_paragraphs,
            &new_visuals_kinds,
            &std::collections::HashMap::new(),
        );
        crate::generation::manim_code::generate_manim_code(
            state,
            user,
            audiobook_id,
            &book.title,
            &book.topic,
            book.genre.as_deref(),
            &chapter.title,
            "library",
            &custom,
        )
        .await
        .into_iter()
        .map(|(k, v)| (k, v.code))
        .collect()
    };

    // Merge: preserve every field that's already on the row; only
    // overlay newly-produced visual_kind / visual_params / manim_code.
    let merged: Vec<serde_json::Value> = existing
        .iter()
        .map(|p| {
            let idx = p.index.max(0) as u32;
            let mut entry = serde_json::Map::new();
            entry.insert("index".into(), serde_json::json!(idx));
            entry.insert("text".into(), serde_json::json!(p.text));
            let cc = p
                .char_count
                .unwrap_or_else(|| p.text.chars().count() as i64);
            entry.insert("char_count".into(), serde_json::json!(cc));
            entry.insert(
                "scene_description".into(),
                serde_json::json!(p.scene_description.clone()),
            );
            entry.insert(
                "image_paths".into(),
                serde_json::json!(p.image_paths.clone()),
            );
            // visual_kind / visual_params: prefer freshly produced,
            // fall back to whatever was already on the row.
            if let Some(v) = visuals.get(&idx) {
                entry.insert("visual_kind".into(), serde_json::json!(v.visual_kind));
                entry.insert("visual_params".into(), v.visual_params.clone());
            } else if let Some(kind) = p.visual_kind.as_deref() {
                if !kind.trim().is_empty() {
                    entry.insert("visual_kind".into(), serde_json::json!(kind));
                    if let Some(params) = p.visual_params.as_ref() {
                        entry.insert("visual_params".into(), params.clone());
                    }
                }
            }
            // manim_code: prefer freshly generated, fall back to
            // whatever was already there. Empty/whitespace drops on
            // the floor — publisher reads missing as "use prose".
            if let Some(code) = manim_codes.get(&idx) {
                if !code.trim().is_empty() {
                    entry.insert("manim_code".into(), serde_json::json!(code));
                }
            } else if let Some(prev) = p.manim_code.as_deref() {
                if !prev.trim().is_empty() {
                    entry.insert("manim_code".into(), serde_json::json!(prev));
                }
            }
            serde_json::Value::Object(entry)
        })
        .collect();

    if let Err(e) = crate::generation::paragraphs::persist(state, &chapter_id, merged).await {
        warn!(
            error = %e,
            audiobook = %audiobook_id,
            chapter = chapter_number,
            "animate classify: persist (partial) failed"
        );
    }
}

/// Helper for branch A: run code-gen for all paragraphs the visual
/// classifier just labelled `custom_manim`. Mirrors the equivalent
/// step in the `chapter_paragraphs` job. Empty when no paragraphs got
/// the custom escape hatch.
async fn run_code_gen_for_custom_manim(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    book: &BookCtx,
    chapter_title: &str,
    paragraphs: &[crate::generation::paragraphs::Paragraph],
    visuals: &std::collections::HashMap<u32, crate::generation::paragraphs::ParagraphVisual>,
) -> std::collections::HashMap<u32, String> {
    if !visuals.values().any(|v| v.visual_kind == "custom_manim") {
        return std::collections::HashMap::new();
    }
    let kinds_only: std::collections::HashMap<u32, String> = visuals
        .iter()
        .map(|(k, v)| (*k, v.visual_kind.clone()))
        .collect();
    let custom = crate::generation::manim_code::custom_paragraphs(
        paragraphs,
        &kinds_only,
        &std::collections::HashMap::new(),
    );
    crate::generation::manim_code::generate_manim_code(
        state,
        user,
        audiobook_id,
        &book.title,
        &book.topic,
        book.genre.as_deref(),
        chapter_title,
        "library",
        &custom,
    )
    .await
    .into_iter()
    .map(|(k, v)| (k, v.code))
    .collect()
}
