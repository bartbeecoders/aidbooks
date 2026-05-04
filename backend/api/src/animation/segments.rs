//! Per-segment chapter renderer (Phase G.6).
//!
//! Used when a chapter is STEM (`is_stem=true`) **and** the fast
//! path is on (`animate_fast_path=true`) **and** at least one scene
//! carries a `visual_kind`. In every other case the publisher
//! routes to the plain `fast_path::render` (single ffmpeg per
//! chapter) or the Revideo pool — segments-mode is opt-in and
//! diagram-driven only.
//!
//! Algorithm:
//!
//!   1. Walk `spec.scenes` in order. For each scene, render a
//!      *video-only* MP4 segment to a tempdir:
//!        * `Paragraph` with `visual_kind` set → Manim sidecar.
//!        * Anything else → single-scene fast-path mini-render.
//!   2. ffmpeg-concat all segments (`-f concat -i list.txt -c copy`)
//!      into a chapter-length video-only MP4.
//!   3. ffmpeg-mux the chapter audio (`ch-N.wav`) into the concat
//!      output and write the final `ch-N.video.mp4`.
//!
//! Segments share the same encode params (libx264 / NVENC / VAAPI /
//! QSV per `hwenc::Encoder`, yuv420p, the project's fps), so the
//! concat step is `-c copy` — no re-encode, near-free even on big
//! chapters.
//!
//! Why not extend `fast_path::render` directly? The single-shot
//! path's filter graph is built around one ffmpeg invocation with
//! one input. Splitting into per-scene mini-invocations is a
//! different orchestration shape; keeping them in their own module
//! avoids drowning the simpler path in branches.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::fast_path;
use super::manim_sidecar::{ManimRendererPool, ManimRequest};
use super::sidecar::RenderFailure;
use super::spec::{Background, Scene, SceneSpec};

/// Which segment is currently rendering. Lets the publisher form
/// a more informative `step` label ("rendering diagram 3/12") than
/// the bare percentage we used before, so the WebSocket-driven UI
/// can tell the user *what* is taking time, not just *how far in*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Title,
    /// `Scene::Paragraph` with `visual_kind` set + Manim pool present.
    /// Slow (LaTeX warmup + full Manim render); the UI signals to the
    /// user that progress is bottlenecked on Manim, not stuck.
    Diagram,
    /// `Scene::Paragraph` with no `visual_kind`, OR with one but the
    /// Manim pool is `None` (config missing) — both render via the
    /// fast-path mini-render. Distinguishing them in the label would
    /// confuse users; both are "paragraph N/M" from their POV.
    Prose,
    Outro,
}

impl SegmentKind {
    /// Human label used in the progress step string. Kept short so
    /// the UI status line stays compact.
    pub fn label(&self) -> &'static str {
        match self {
            SegmentKind::Title => "title",
            SegmentKind::Diagram => "diagram",
            SegmentKind::Prose => "paragraph",
            SegmentKind::Outro => "outro",
        }
    }
}

/// Emitted once per rendered segment. Cheap to clone; the publisher
/// drain forms the step label and forwards a fraction to ctx.progress.
#[derive(Debug, Clone, Copy)]
pub struct SegmentProgress {
    /// Just-rendered segment index, 0-based.
    pub index: u32,
    /// Total segment count for this chapter.
    pub total: u32,
    pub kind: SegmentKind,
    /// Overall render progress, 0.0–1.0. Reaches `1.0` only after
    /// concat + audio mux complete; segment-loop emissions cap at 0.9.
    pub fraction: f32,
}

/// Render a chapter via the per-segment pipeline.
///
/// `manim_pool` is `None` in mock mode or when no Manim sidecar
/// command is configured — diagram scenes in that case fall back to
/// prose rendering (the fast path for that scene's text), which
/// preserves chapter timing but loses the diagram visual. A warn
/// log on each fallback so the operator notices.
pub async fn render_chapter(
    spec: &SceneSpec,
    ffmpeg_bin: &str,
    hwenc_override: &str,
    vaapi_device: &str,
    manim_pool: Option<Arc<ManimRendererPool>>,
    progress: mpsc::UnboundedSender<SegmentProgress>,
) -> Result<(), RenderFailure> {
    let bin = if ffmpeg_bin.trim().is_empty() {
        "ffmpeg"
    } else {
        ffmpeg_bin
    };

    let final_mp4 = spec.output.mp4.clone();
    let parent = final_mp4
        .parent()
        .ok_or_else(|| {
            RenderFailure::Fatal(format!(
                "output.mp4 `{}` has no parent dir",
                final_mp4.display()
            ))
        })?
        .to_path_buf();

    // Scratch dir for per-scene segments + concat list. Sibling to
    // the final mp4 so a failed render leaves debuggable artefacts.
    let stem = final_mp4
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video");
    let scratch = parent.join(format!("{stem}.segments-tmp"));
    std::fs::create_dir_all(&scratch).map_err(|e| {
        RenderFailure::Transient(format!("create segments scratch dir: {e}"))
    })?;

    let total_scenes = spec.scenes.len().max(1);
    let mut segment_paths: Vec<PathBuf> = Vec::with_capacity(spec.scenes.len());

    for (i, scene) in spec.scenes.iter().enumerate() {
        let seg_path = scratch.join(format!("seg-{i:03}.mp4"));

        // Decide the segment kind *before* rendering so the
        // SegmentProgress label reflects what was rendered, not
        // the next scene's shape. `Diagram` only when we'd
        // actually route through Manim — otherwise it's `Prose`
        // (the user shouldn't see "diagram" in the status when we
        // silently fell back to prose).
        let kind = match scene {
            Scene::Title { .. } => SegmentKind::Title,
            Scene::Outro { .. } => SegmentKind::Outro,
            // `custom_manim` is only diagram-routed when the code
            // has been generated; missing/empty code falls back to
            // prose, so the label needs to reflect that fallback so
            // the UI doesn't claim "diagram 3/12" for what really
            // rendered as plain text.
            Scene::Paragraph {
                visual_kind: Some(k),
                manim_code,
                ..
            } if manim_pool.is_some()
                && (k != "custom_manim"
                    || manim_code.as_deref().is_some_and(|c| !c.trim().is_empty())) =>
            {
                SegmentKind::Diagram
            }
            Scene::Paragraph { .. } => SegmentKind::Prose,
        };

        let result = match scene {
            // Phase H — custom_manim with persisted code goes through
            // the sidecar's raw_scene path. If `manim_code` is empty
            // we fall through to the warn-and-prose branch below
            // (same as a structured visual_kind without a Manim pool).
            Scene::Paragraph {
                visual_kind: Some(kind),
                manim_code: Some(code),
                start_ms,
                end_ms,
                ..
            } if kind == "custom_manim" && manim_pool.is_some() && !code.trim().is_empty() => {
                let duration_ms = end_ms.saturating_sub(*start_ms).max(1);
                let req = ManimRequest::RawScene {
                    code: code.clone(),
                    duration_ms,
                    output_mp4: seg_path.clone(),
                };
                let pool = manim_pool.as_ref().unwrap();
                pool.render(&req).await
            }
            Scene::Paragraph {
                visual_kind: Some(kind),
                visual_params,
                start_ms,
                end_ms,
                ..
            } if kind != "custom_manim" && manim_pool.is_some() => {
                let duration_ms = end_ms.saturating_sub(*start_ms).max(1);
                let req = ManimRequest::Template {
                    template_id: kind.clone(),
                    params: visual_params.clone().unwrap_or(serde_json::json!({})),
                    duration_ms,
                    output_mp4: seg_path.clone(),
                };
                let pool = manim_pool.as_ref().unwrap();
                pool.render(&req).await
            }
            Scene::Paragraph {
                visual_kind: Some(kind),
                ..
            } => {
                // visual_kind is set but Manim isn't configured —
                // fall back to prose. Log so operators notice. The
                // `custom_manim` case also lands here when manim_code
                // is missing (LLM hasn't run yet) or empty (LLM
                // declined — prose is the right fallback).
                warn!(
                    visual_kind = kind,
                    "segments: Manim sidecar not configured (or custom_manim code missing); rendering prose fallback"
                );
                render_prose_segment(
                    spec,
                    scene,
                    &seg_path,
                    bin,
                    hwenc_override,
                    vaapi_device,
                )
                .await
            }
            _ => {
                render_prose_segment(
                    spec,
                    scene,
                    &seg_path,
                    bin,
                    hwenc_override,
                    vaapi_device,
                )
                .await
            }
        };

        match result {
            Ok(()) => {}
            Err(e) => {
                // Leave scratch in place for diagnosis.
                return Err(e);
            }
        }
        if !seg_path.exists() {
            return Err(RenderFailure::Transient(format!(
                "segment {i} render reported success but no file at {}",
                seg_path.display()
            )));
        }

        segment_paths.push(seg_path);

        // Coarse progress: i+1 segments out of N rendered, scaled
        // to the 0.0–0.9 band so the final concat + mux fills the
        // last 10 %.
        let frac = ((i + 1) as f32 / total_scenes as f32) * 0.9;
        let _ = progress.send(SegmentProgress {
            index: i as u32,
            total: total_scenes as u32,
            kind,
            fraction: frac,
        });
    }

    // ---- concat segments + mux audio ---------------------------------

    let video_only = scratch.join("concat.mp4");
    concat_segments(bin, &segment_paths, &scratch, &video_only).await?;

    // Concat + mux fall outside the per-scene loop. We tag them
    // with the last segment's kind for label continuity (the UI
    // would otherwise flicker the kind label back to "title" for
    // these two sends, since `Outro` is always last only when the
    // scene list has one). The frontend treats concat/mux as
    // a single tail phase via the fraction `>= 0.9`.
    let tail_kind = SegmentKind::Outro;
    let total_u32 = total_scenes as u32;
    let _ = progress.send(SegmentProgress {
        index: total_u32.saturating_sub(1),
        total: total_u32,
        kind: tail_kind,
        fraction: 0.95,
    });

    mux_audio(bin, &video_only, &spec.audio.wav, &final_mp4).await?;

    let _ = progress.send(SegmentProgress {
        index: total_u32.saturating_sub(1),
        total: total_u32,
        kind: tail_kind,
        fraction: 1.0,
    });

    // Clean scratch on success. Failed renders kept everything for
    // debugging; only the happy path nukes the dir.
    if let Err(e) = std::fs::remove_dir_all(&scratch) {
        warn!(error = %e, dir = %scratch.display(), "segments: cleanup failed");
    }

    Ok(())
}

/// Render one non-diagram scene as its own video-only segment via
/// the fast-path's existing helpers. Builds a single-scene
/// `SceneSpec` with the scene retimed to start at 0, then runs the
/// usual fast-path render — minus the audio mux, since we mux
/// once at the chapter level.
async fn render_prose_segment(
    parent_spec: &SceneSpec,
    scene: &Scene,
    seg_path: &Path,
    ffmpeg_bin: &str,
    hwenc_override: &str,
    vaapi_device: &str,
) -> Result<(), RenderFailure> {
    let (start_ms, end_ms) = match scene {
        Scene::Title { start_ms, end_ms, .. }
        | Scene::Paragraph { start_ms, end_ms, .. }
        | Scene::Outro { start_ms, end_ms, .. } => (*start_ms, *end_ms),
    };
    let duration_ms = end_ms.saturating_sub(start_ms).max(1);

    // Re-base the scene at t=0 so its ASS cue + the segment's
    // timeline match. Otherwise libass wouldn't show the cue at
    // all (the cue's `Start` time would be in the future).
    let rebased = retime_scene(scene, 0, duration_ms);

    let mut sub_spec = parent_spec.clone();
    // Replace the scenes list with just this one re-based scene.
    sub_spec.scenes = vec![rebased];
    // Match the segment's runtime to the scene's runtime so
    // libass + zoompan size correctly.
    sub_spec.chapter.duration_ms = duration_ms;
    sub_spec.output.mp4 = seg_path.to_path_buf();
    // Strip captions — they belong to the whole-chapter render, not
    // a single-scene segment.
    sub_spec.captions = None;

    // Render. The fast-path expects a *real* audio input; we build
    // a silent WAV at segment duration to satisfy `-map 1:a`. The
    // segment's audio is discarded at concat time anyway because
    // `mux_audio` overwrites with the chapter WAV.
    let scratch_wav = seg_path.with_extension("silent.wav");
    build_silent_wav(ffmpeg_bin, duration_ms, &scratch_wav).await?;
    sub_spec.audio.wav = scratch_wav.clone();

    let (tx, mut rx) = mpsc::unbounded_channel::<f32>();
    let drain = tokio::spawn(async move {
        // Drain progress events from the sub-render but don't surface
        // them — segments emit their own coarse progress at the
        // chapter level.
        while rx.recv().await.is_some() {}
    });
    let result = fast_path::render(&sub_spec, ffmpeg_bin, hwenc_override, vaapi_device, tx).await;
    let _ = drain.await;

    // Always clean the silent WAV; nothing references it after the render.
    let _ = std::fs::remove_file(&scratch_wav);

    result
}

fn retime_scene(scene: &Scene, start_ms: u64, duration_ms: u64) -> Scene {
    let end_ms = start_ms + duration_ms;
    match scene {
        Scene::Title {
            title, subtitle, ..
        } => Scene::Title {
            start_ms,
            end_ms,
            title: title.clone(),
            subtitle: subtitle.clone(),
        },
        Scene::Paragraph {
            text,
            tile,
            highlight,
            visual_kind,
            visual_params,
            manim_code,
            ..
        } => Scene::Paragraph {
            start_ms,
            end_ms,
            text: text.clone(),
            tile: tile.clone(),
            highlight: highlight.clone(),
            visual_kind: visual_kind.clone(),
            visual_params: visual_params.clone(),
            manim_code: manim_code.clone(),
        },
        Scene::Outro {
            title, subtitle, ..
        } => Scene::Outro {
            start_ms,
            end_ms,
            title: title.clone(),
            subtitle: subtitle.clone(),
        },
    }
}

async fn build_silent_wav(
    ffmpeg_bin: &str,
    duration_ms: u64,
    out: &Path,
) -> Result<(), RenderFailure> {
    let secs = (duration_ms as f64 / 1000.0).max(0.05);
    let mut cmd = Command::new(ffmpeg_bin);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(format!(
            "anullsrc=channel_layout=mono:sample_rate=24000"
        ))
        .arg("-t")
        .arg(format!("{secs:.3}"))
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let out_proc = cmd
        .output()
        .await
        .map_err(|e| RenderFailure::Transient(format!("spawn ffmpeg silent wav: {e}")))?;
    if !out_proc.status.success() {
        let tail = String::from_utf8_lossy(&out_proc.stderr).into_owned();
        return Err(RenderFailure::Transient(format!(
            "ffmpeg silent wav exited with {}: {}",
            out_proc.status,
            tail.trim_end()
        )));
    }
    Ok(())
}

async fn concat_segments(
    ffmpeg_bin: &str,
    segments: &[PathBuf],
    scratch: &Path,
    output: &Path,
) -> Result<(), RenderFailure> {
    if segments.is_empty() {
        return Err(RenderFailure::Fatal(
            "concat_segments: no segments to concat".into(),
        ));
    }

    let list_path = scratch.join("concat-list.txt");
    let mut list = String::new();
    for seg in segments {
        // ffmpeg's concat demuxer needs file paths quoted (single
        // quotes) to tolerate special chars. Escape any embedded
        // single quote per its manual: `'\''`.
        let quoted = seg
            .to_string_lossy()
            .replace('\'', r"'\''");
        list.push_str(&format!("file '{quoted}'\n"));
    }
    std::fs::write(&list_path, list).map_err(|e| {
        RenderFailure::Transient(format!("write concat list: {e}"))
    })?;

    let mut cmd = Command::new(ffmpeg_bin);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&list_path)
        .arg("-c")
        .arg("copy")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let stderr_log = run_and_collect_stderr(cmd).await?;
    if !output.exists() {
        return Err(RenderFailure::Transient(format!(
            "ffmpeg concat reported success but no output at {}\nstderr: {}",
            output.display(),
            stderr_log.trim_end()
        )));
    }
    Ok(())
}

async fn mux_audio(
    ffmpeg_bin: &str,
    video_only: &Path,
    audio_wav: &Path,
    output: &Path,
) -> Result<(), RenderFailure> {
    let mut cmd = Command::new(ffmpeg_bin);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(video_only)
        .arg("-i")
        .arg(audio_wav)
        .arg("-map")
        .arg("0:v:0")
        .arg("-map")
        .arg("1:a:0")
        .arg("-c:v")
        .arg("copy")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-shortest")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let stderr_log = run_and_collect_stderr(cmd).await?;
    if !output.exists() {
        return Err(RenderFailure::Transient(format!(
            "ffmpeg mux_audio reported success but no output at {}\nstderr: {}",
            output.display(),
            stderr_log.trim_end()
        )));
    }
    Ok(())
}

/// Run a `Command` to completion, surface stderr on failure as part
/// of the error message + return as a string for inspection.
async fn run_and_collect_stderr(mut cmd: Command) -> Result<String, RenderFailure> {
    let mut child = cmd
        .spawn()
        .map_err(|e| RenderFailure::Transient(format!("spawn ffmpeg: {e}")))?;
    let stderr = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        if let Some(stderr) = stderr {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                debug!(target: "animate.segments", "{}", line);
                if buf.len() < 4_096 {
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
        }
        buf
    });
    let status = child
        .wait()
        .await
        .map_err(|e| RenderFailure::Transient(format!("await ffmpeg: {e}")))?;
    let stderr_buf = stderr_task.await.unwrap_or_default();
    if !status.success() {
        return Err(RenderFailure::Transient(format!(
            "ffmpeg exited with {}: {}",
            status,
            stderr_buf.trim_end()
        )));
    }
    Ok(stderr_buf)
}

/// True when `spec` has at least one paragraph scene with a
/// `visual_kind` set — i.e. the per-segment renderer would actually
/// add value over the single-shot fast-path. Used by the publisher
/// to decide whether to take this path at all.
pub fn has_diagram_scenes(spec: &SceneSpec) -> bool {
    spec.scenes.iter().any(|s| {
        matches!(
            s,
            Scene::Paragraph {
                visual_kind: Some(_),
                ..
            }
        )
    })
}

// silence dead-code warnings while the publisher branch is being
// wired up — referenced from animate.rs once G.6.d lands.
#[allow(dead_code)]
fn _touch_background(_b: &Background) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::spec::{
        AudioRef, Background, ChapterMeta, Output, Scene, SceneSpec, Theme,
    };

    fn fixture(visual_kind: Option<&str>) -> SceneSpec {
        let mut spec = SceneSpec::new(
            ChapterMeta {
                number: 1,
                title: "T".into(),
                duration_ms: 10_000,
            },
            AudioRef {
                wav: PathBuf::from("/tmp/x.wav"),
                peaks: None,
            },
            Background::Color {
                color: "#0F172A".into(),
            },
            Output::hd_1080(PathBuf::from("/tmp/out.mp4"), 24),
        )
        .with_theme(Theme {
            preset: "library".into(),
            primary: None,
            accent: None,
        });
        spec.scenes.push(Scene::Title {
            start_ms: 0,
            end_ms: 4_000,
            title: "Hi".into(),
            subtitle: None,
        });
        spec.scenes.push(Scene::Paragraph {
            start_ms: 4_000,
            end_ms: 9_000,
            text: "Body".into(),
            tile: None,
            highlight: "karaoke".into(),
            visual_kind: visual_kind.map(str::to_string),
            visual_params: visual_kind.map(|_| serde_json::json!({"fn": "x"})),
            manim_code: None,
        });
        spec.scenes.push(Scene::Outro {
            start_ms: 9_000,
            end_ms: 10_000,
            title: "End".into(),
            subtitle: None,
        });
        spec
    }

    #[test]
    fn has_diagram_scenes_detects_visual_kind() {
        assert!(has_diagram_scenes(&fixture(Some("function_plot"))));
        assert!(!has_diagram_scenes(&fixture(None)));
    }

    #[test]
    fn retime_scene_preserves_paragraph_fields() {
        let scene = Scene::Paragraph {
            start_ms: 4_000,
            end_ms: 9_000,
            text: "abc".into(),
            tile: Some(PathBuf::from("/tmp/p.webp")),
            highlight: "karaoke".into(),
            visual_kind: Some("function_plot".into()),
            visual_params: Some(serde_json::json!({"fn": "x**2"})),
            manim_code: None,
        };
        let rebased = retime_scene(&scene, 0, 5_000);
        match rebased {
            Scene::Paragraph {
                start_ms,
                end_ms,
                visual_kind,
                visual_params,
                tile,
                ..
            } => {
                assert_eq!(start_ms, 0);
                assert_eq!(end_ms, 5_000);
                assert_eq!(visual_kind.as_deref(), Some("function_plot"));
                assert!(visual_params.is_some());
                assert!(tile.is_some());
            }
            _ => panic!("retime should preserve scene kind"),
        }
    }
}
