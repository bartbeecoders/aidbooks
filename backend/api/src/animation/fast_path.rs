//! ffmpeg-only fast path (Phase F.1c).
//!
//! Skip Chromium / Revideo / Vite for chapters where we can express
//! the visual layout entirely as a single ffmpeg invocation:
//!
//!   * **Background**: Ken Burns zoom on the cover image, or a solid
//!     colour fill via `lavfi`.
//!   * **Text**: every scene's text body is emitted as a styled cue in
//!     an auto-generated ASS subtitle file. libass renders it with
//!     theme-aware fonts/colours/positioning.
//!   * **Audio**: chapter WAV muxed in directly, encoded to AAC.
//!
//! v1 trade-offs vs the Revideo path (documented + intentional):
//!
//!   * No animated title underline draw.
//!   * No per-paragraph tile image overlays.
//!   * No per-word karaoke reveal — text appears on cue, holds, fades
//!     out (controlled by ASS fade tags).
//!   * No waveform-pulse accent strip.
//!   * Hard cuts between scenes (no crossfades).
//!
//! What we get back: a 5–10× speedup on paragraph-dominant chapters
//! because the entire pipeline is libavcodec / libavfilter — no
//! Chromium boot, no Vite, no JS at all.

use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, OnceCell};
use tracing::{debug, warn};

use super::hwenc::{self, Encoder};
use super::sidecar::RenderFailure;
use super::spec::{Background, Scene, SceneSpec};

/// Process-local cache of the detected encoder. Probed once at
/// first render; subsequent calls return the cached choice. Restart
/// the process to pick up env changes — keyed config drift would
/// require a more complex cache than is warranted here.
static ENCODER_CACHE: OnceLock<OnceCell<Encoder>> = OnceLock::new();

async fn cached_encoder(ffmpeg_bin: &str, override_choice: &str, vaapi_device: &str) -> Encoder {
    let cell = ENCODER_CACHE.get_or_init(OnceCell::new);
    *cell
        .get_or_init(|| {
            let bin = ffmpeg_bin.to_string();
            let ovr = override_choice.to_string();
            let dev = vaapi_device.to_string();
            async move { hwenc::detect(&bin, &ovr, &dev).await }
        })
        .await
}

/// Built-in theme palettes. Mirrors the renderer's
/// `backend/render/src/themes/index.ts` so the fast path produces
/// visually-similar output without re-reading the JS source. If a
/// preset is unknown, falls back to `library`.
struct ThemePalette {
    background_hex: &'static str,
    accent_hex: &'static str,
    text_hex: &'static str,
    font_name: &'static str,
}

const LIBRARY: ThemePalette = ThemePalette {
    background_hex: "#0F172A",
    accent_hex: "#F59E0B",
    text_hex: "#FFFFFF",
    font_name: "Inter",
};
const PARCHMENT: ThemePalette = ThemePalette {
    background_hex: "#44403C",
    accent_hex: "#B45309",
    text_hex: "#FAF6E8",
    font_name: "Inter",
};
const MINIMAL: ThemePalette = ThemePalette {
    background_hex: "#18181B",
    accent_hex: "#FFFFFF",
    text_hex: "#FFFFFF",
    font_name: "Inter",
};

fn palette_for(preset: &str) -> &'static ThemePalette {
    match preset {
        "parchment" => &PARCHMENT,
        "minimal" => &MINIMAL,
        _ => &LIBRARY,
    }
}

/// Run a single fast-path render. Writes an ASS sidecar + an ffmpeg
/// filter-graph script to the output mp4's directory, runs ffmpeg
/// once, then deletes both intermediates on success. Failure paths
/// leave them in place so the operator can re-run by hand to
/// reproduce.
///
/// `hwenc_override` mirrors `Config::animate_hwenc` and `vaapi_device`
/// mirrors `Config::animate_vaapi_device`. Both are passed in by the
/// caller so the publisher owns the config plumbing and this
/// function stays self-contained for testing. Detection runs once
/// per process and the result is cached.
pub async fn render(
    spec: &SceneSpec,
    ffmpeg_bin: &str,
    hwenc_override: &str,
    vaapi_device: &str,
    progress: mpsc::UnboundedSender<f32>,
) -> Result<(), RenderFailure> {
    let bin = if ffmpeg_bin.trim().is_empty() {
        "ffmpeg"
    } else {
        ffmpeg_bin
    };
    let encoder = cached_encoder(bin, hwenc_override, vaapi_device).await;

    let out_path = spec.output.mp4.clone();
    let parent = out_path
        .parent()
        .ok_or_else(|| {
            RenderFailure::Fatal(format!(
                "output.mp4 `{}` has no parent dir",
                out_path.display()
            ))
        })?
        .to_path_buf();

    // Sidecar files live next to the MP4 with deterministic names so
    // a failed render leaves debuggable artefacts behind.
    let stem = out_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video");
    let ass_path = parent.join(format!("{stem}.fastpath.ass"));
    let filter_path = parent.join(format!("{stem}.fastpath.filter"));

    let palette = palette_for(&spec.theme.preset);
    let ass = build_ass(spec, palette);
    std::fs::write(&ass_path, ass).map_err(|e| {
        RenderFailure::Transient(format!("write ASS file {}: {e}", ass_path.display()))
    })?;

    let filter_graph = build_filter_graph(spec, &ass_path, encoder)?;
    std::fs::write(&filter_path, &filter_graph).map_err(|e| {
        RenderFailure::Transient(format!(
            "write filter-graph script {}: {e}",
            filter_path.display()
        ))
    })?;

    // Build the ffmpeg invocation. Two input cases:
    //   * Image background → `-loop 1 -framerate F -t D -i cover`
    //   * Colour background → `-f lavfi -i color=c=...:s=...:r=F:d=D`
    // Both leave [0:v] available to the filter graph; the audio is
    // always input 1 so the `-map 1:a` below is stable.
    let duration_secs = (spec.chapter.duration_ms as f64 / 1000.0).max(0.1);
    let fps = spec.output.fps.max(1);
    let width = spec.output.width.max(1);
    let height = spec.output.height.max(1);

    let mut cmd = Command::new(bin);
    cmd.arg("-y")
        .arg("-hide_banner")
        // Progress comes from -progress on stderr; everything else is
        // verbose noise we don't need.
        .arg("-loglevel")
        .arg("error");

    // VAAPI needs `-init_hw_device` + `-filter_hw_device` BEFORE the
    // first `-i`; every other encoder leaves this empty.
    for arg in hwenc::pre_input_args(encoder, vaapi_device) {
        cmd.arg(arg);
    }

    match &spec.background {
        Background::Image { src, .. } => {
            cmd.arg("-loop")
                .arg("1")
                .arg("-framerate")
                .arg(fps.to_string())
                .arg("-t")
                .arg(format!("{duration_secs:.3}"))
                .arg("-i")
                .arg(src);
        }
        Background::Color { color } => {
            cmd.arg("-f").arg("lavfi").arg("-i").arg(format!(
                "color=c={color}:s={width}x{height}:r={fps}:d={duration_secs:.3}"
            ));
        }
    }

    // Audio input always lands at index 1.
    cmd.arg("-i").arg(&spec.audio.wav);

    cmd.arg("-filter_complex_script")
        .arg(&filter_path)
        .arg("-map")
        .arg("[v_out]")
        .arg("-map")
        .arg("1:a");

    // Per-encoder video args (libx264 / h264_nvenc / h264_vaapi /
    // h264_qsv). The fast path's pixel format is yuv420p coming out
    // of libass; software/NVENC/QSV encoders accept that directly,
    // VAAPI's filter tail has already converted it to nv12 + uploaded
    // to GPU memory.
    for arg in hwenc::encoder_args(encoder) {
        cmd.arg(arg);
    }
    if encoder != Encoder::Vaapi {
        // VAAPI encoder pixel-fmt is owned by the hwupload chain; the
        // CPU encoders need the explicit hint.
        cmd.arg("-pix_fmt").arg("yuv420p");
    }

    cmd.arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-movflags")
        .arg("+faststart")
        .arg("-shortest")
        // Progress on stderr in `key=value` lines, one per ~500 ms.
        .arg("-progress")
        .arg("pipe:2")
        .arg("-nostats")
        .arg(&out_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| RenderFailure::Transient(format!("spawn ffmpeg (fast path): {e}")))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| RenderFailure::Transient("ffmpeg fast path stderr not piped".into()))?;
    let total_frames = ((spec.chapter.duration_ms as f64 / 1000.0) * fps as f64).max(1.0) as u64;

    let progress_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        let mut tail = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            // -progress writes `frame=N`, `out_time_ms=...`,
            // `progress=continue|end`, etc. We just want frame= for
            // a 0..1 fraction.
            if let Some(rest) = line.strip_prefix("frame=") {
                if let Ok(n) = rest.trim().parse::<u64>() {
                    let frac = (n as f32 / total_frames as f32).clamp(0.0, 1.0);
                    let _ = progress.send(frac);
                    continue;
                }
            }
            // Anything else (warnings, real errors that slip through
            // -loglevel error) goes to a rolling tail for diagnostics.
            debug!(target: "animate.fast_path", "{}", line);
            if tail.len() < 4_096 {
                tail.push_str(&line);
                tail.push('\n');
            }
        }
        tail
    });

    let status = child
        .wait()
        .await
        .map_err(|e| RenderFailure::Transient(format!("await ffmpeg fast path: {e}")))?;
    let stderr_tail = progress_task.await.unwrap_or_default();

    if !status.success() {
        return Err(RenderFailure::Transient(format!(
            "ffmpeg fast path exited with {}: {}",
            status,
            stderr_tail.trim_end()
        )));
    }

    // Clean up intermediates on success.
    if let Err(e) = std::fs::remove_file(&ass_path) {
        warn!(error = %e, path = %ass_path.display(), "fast_path: cleanup ass file failed");
    }
    if let Err(e) = std::fs::remove_file(&filter_path) {
        warn!(error = %e, path = %filter_path.display(), "fast_path: cleanup filter file failed");
    }

    Ok(())
}

/// Build the ffmpeg filter graph as a string suitable for
/// `-filter_complex_script`.
///
/// For an image background:
///   `[0:v]scale=2400:1350,zoompan=...,subtitles=...[v_out]`
///
/// For a colour background (already correctly sized by the lavfi
/// source):
///   `[0:v]subtitles=...[v_out]`
///
/// VAAPI appends `,format=nv12,hwupload` before the `[v_out]` label
/// so the rasterised frame moves to GPU memory before the encoder
/// consumes it. libass + the `subtitles` filter both run on CPU
/// pixel data, so the upload has to land *after* the text composite.
fn build_filter_graph(
    spec: &SceneSpec,
    ass_path: &Path,
    encoder: Encoder,
) -> Result<String, RenderFailure> {
    let total_frames =
        ((spec.chapter.duration_ms as f64 / 1000.0) * spec.output.fps as f64).max(1.0) as u64;
    let width = spec.output.width.max(1);
    let height = spec.output.height.max(1);

    // Per-frame zoom step that lands on 1.10x exactly at the last
    // frame — a soft Ken Burns over the full chapter rather than
    // ratcheting up early and plateauing.
    let zoom_step = 0.10 / total_frames as f64;

    let escaped_ass = escape_for_filter_path(&ass_path.to_string_lossy());
    let hw_tail = hwenc::filter_graph_tail(encoder);

    let mut graph = String::new();
    match &spec.background {
        Background::Image { kenburns, .. } => {
            // Up-scale the image so the zoom doesn't reveal pixel
            // edges, then zoompan, then subtitles. `force_original_
            // aspect_ratio=increase` + `crop` keeps the aspect right
            // even if the cover is the wrong shape.
            graph.push_str(&format!(
                "[0:v]scale={ow}:{oh}:force_original_aspect_ratio=increase,crop={ow}:{oh}",
                ow = width as u64 * 5 / 4,
                oh = height as u64 * 5 / 4,
            ));
            if *kenburns {
                graph.push_str(&format!(
                    ",zoompan=z='min(zoom+{zoom_step:.10},1.10)':d={total_frames}:s={width}x{height}:fps={fps}",
                    fps = spec.output.fps.max(1)
                ));
            } else {
                graph.push_str(&format!(",scale={width}:{height}"));
            }
            graph.push_str(&format!(
                ",format=yuv420p,subtitles='{escaped_ass}'{hw_tail}[v_out]"
            ));
        }
        Background::Color { .. } => {
            graph.push_str(&format!(
                "[0:v]format=yuv420p,subtitles='{escaped_ass}'{hw_tail}[v_out]"
            ));
        }
    }
    Ok(graph)
}

/// Build the ASS subtitle file body. One `Dialogue` cue per scene,
/// styled by scene kind.
fn build_ass(spec: &SceneSpec, palette: &ThemePalette) -> String {
    let title_colour = ass_colour(palette.text_hex);
    let para_colour = ass_colour(palette.text_hex);
    let outro_colour = ass_colour(palette.accent_hex);
    let outline = ass_colour(palette.background_hex);
    // 50 % alpha (`80`) shadow, fully opaque background — keeps text
    // legible over busy cover images.
    let back = format!("&H80{}", &outline.trim_start_matches("&H00"));
    let font = palette.font_name;
    let width = spec.output.width.max(1);
    let height = spec.output.height.max(1);

    let mut s = String::new();
    s.push_str("[Script Info]\n");
    s.push_str("ScriptType: v4.00+\n");
    s.push_str(&format!("PlayResX: {width}\n"));
    s.push_str(&format!("PlayResY: {height}\n"));
    s.push_str("WrapStyle: 0\n");
    s.push_str("ScaledBorderAndShadow: yes\n");
    s.push_str("YCbCr Matrix: TV.709\n\n");

    s.push_str("[V4+ Styles]\n");
    s.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");

    // Alignment numbers (numpad-style):
    //   8 = top-center, 5 = middle-center, 2 = bottom-center.
    // Title sits high; paragraphs fill the middle band; outro sits a
    // hair higher than centre to leave room for the CTA subtitle.
    s.push_str(&format!(
        "Style: Title,{font},78,{title_colour},{outline},{back},1,0,0,0,100,100,0,0,1,3,2,8,80,80,180,1\n"
    ));
    s.push_str(&format!(
        "Style: Paragraph,{font},48,{para_colour},{outline},{back},0,0,0,0,100,100,0,0,1,2,2,5,140,140,0,1\n"
    ));
    s.push_str(&format!(
        "Style: Outro,{font},66,{outro_colour},{outline},{back},1,0,0,0,100,100,0,0,1,3,2,5,80,80,0,1\n\n"
    ));

    s.push_str("[Events]\n");
    s.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    for scene in &spec.scenes {
        match scene {
            Scene::Title {
                start_ms,
                end_ms,
                title,
                subtitle,
            } => {
                let body = match subtitle {
                    Some(sub) if !sub.trim().is_empty() => {
                        format!("{}\\N{}", escape_ass(title), escape_ass(sub))
                    }
                    _ => escape_ass(title),
                };
                s.push_str(&format!(
                    "Dialogue: 0,{},{},Title,,0,0,0,,{{\\fad(400,400)}}{}\n",
                    ass_time(*start_ms),
                    ass_time(*end_ms),
                    body
                ));
            }
            Scene::Paragraph {
                start_ms,
                end_ms,
                text,
                ..
            } => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                s.push_str(&format!(
                    "Dialogue: 0,{},{},Paragraph,,0,0,0,,{{\\fad(300,300)}}{}\n",
                    ass_time(*start_ms),
                    ass_time(*end_ms),
                    escape_ass(trimmed)
                ));
            }
            Scene::Outro {
                start_ms,
                end_ms,
                title,
                subtitle,
            } => {
                let body = match subtitle {
                    Some(sub) if !sub.trim().is_empty() => {
                        format!("{}\\N{}", escape_ass(title), escape_ass(sub))
                    }
                    _ => escape_ass(title),
                };
                s.push_str(&format!(
                    "Dialogue: 0,{},{},Outro,,0,0,0,,{{\\fad(400,400)}}{}\n",
                    ass_time(*start_ms),
                    ass_time(*end_ms),
                    body
                ));
            }
        }
    }
    s
}

/// Convert a CSS-style `#RRGGBB` to ASS-style `&H00BBGGRR`. ASS uses
/// little-endian channel order with an alpha byte first; our
/// content is fully opaque (`00`).
fn ass_colour(hex: &str) -> String {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        // Fallback to white so libass doesn't reject the file outright.
        return "&H00FFFFFF".into();
    }
    let r = &h[0..2];
    let g = &h[2..4];
    let b = &h[4..6];
    format!(
        "&H00{}{}{}",
        b.to_uppercase(),
        g.to_uppercase(),
        r.to_uppercase()
    )
}

/// Format milliseconds as ASS `H:MM:SS.cc`.
fn ass_time(ms: u64) -> String {
    let total_cs = ms / 10;
    let cs = total_cs % 100;
    let total_secs = total_cs / 100;
    let s = total_secs % 60;
    let total_mins = total_secs / 60;
    let m = total_mins % 60;
    let h = total_mins / 60;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

/// Escape an arbitrary string for an ASS Dialogue Text field. ASS
/// uses `{...}` for inline override tags and `\N` for hard newlines;
/// we strip the former so user text can't accidentally inject style
/// changes, and convert real newlines to `\N`.
fn escape_ass(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '{' | '}' => out.push(' '),
            '\n' => out.push_str("\\N"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Escape a path so it lands inside an ffmpeg `subtitles=` argument
/// without breaking the filter parser. ffmpeg uses `\` and `:` and
/// `'` as syntax in filter args; `subtitles` filenames specifically
/// need `:` and `\` doubled. Quote with single quotes so spaces are
/// safe.
fn escape_for_filter_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len() + 8);
    for ch in p.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ':' => out.push_str("\\:"),
            '\'' => out.push_str("\\'"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::animation::spec::{
        AudioRef, Background, ChapterMeta, Output, Scene, SceneSpec, Theme,
    };

    fn fixture(out: &str) -> SceneSpec {
        SceneSpec::new(
            ChapterMeta {
                number: 3,
                title: "The Trust Stack".into(),
                duration_ms: 30_000,
            },
            AudioRef {
                wav: PathBuf::from("/tmp/ch-3.wav"),
                peaks: None,
            },
            Background::Image {
                src: PathBuf::from("/tmp/cover.webp"),
                kenburns: true,
            },
            Output::hd_1080(PathBuf::from(out), 24),
        )
        .with_theme(Theme {
            preset: "library".into(),
            primary: None,
            accent: None,
        })
        .push(Scene::Title {
            start_ms: 0,
            end_ms: 4_000,
            title: "Chapter 3".into(),
            subtitle: Some("The Trust Stack".into()),
        })
        .push(Scene::Paragraph {
            start_ms: 4_000,
            end_ms: 27_000,
            text: "Trust is the substrate of every transaction.".into(),
            tile: None,
            highlight: "karaoke".into(),
            visual_kind: None,
            visual_params: None,
            manim_code: None,
        })
        .push(Scene::Outro {
            start_ms: 27_000,
            end_ms: 30_000,
            title: "Continue listening".into(),
            subtitle: Some("listenai.app".into()),
        })
    }

    #[test]
    fn ass_colour_swaps_rgb_to_bgr() {
        // #F59E0B (amber) — R=F5 G=9E B=0B → &H000B9EF5.
        assert_eq!(ass_colour("#F59E0B"), "&H000B9EF5");
        // White stays white.
        assert_eq!(ass_colour("#FFFFFF"), "&H00FFFFFF");
        // Bad input falls back to white rather than rejecting.
        assert_eq!(ass_colour("not-a-colour"), "&H00FFFFFF");
    }

    #[test]
    fn ass_time_formats_h_mm_ss_cc() {
        assert_eq!(ass_time(0), "0:00:00.00");
        assert_eq!(ass_time(1_234), "0:00:01.23");
        assert_eq!(ass_time(65_500), "0:01:05.50");
        assert_eq!(ass_time(3_605_010), "1:00:05.01");
    }

    #[test]
    fn escape_ass_strips_curly_braces_and_normalises_newlines() {
        assert_eq!(escape_ass("hello\n{tag}world"), "hello\\N tag world");
    }

    #[test]
    fn escape_for_filter_path_doubles_colons_and_quotes() {
        assert_eq!(
            escape_for_filter_path("/tmp/foo:bar/'baz'.ass"),
            "/tmp/foo\\:bar/\\'baz\\'.ass"
        );
    }

    #[test]
    fn build_ass_includes_one_dialogue_per_scene() {
        let spec = fixture("/tmp/out.mp4");
        let ass = build_ass(&spec, palette_for("library"));
        let cues = ass.matches("Dialogue: ").count();
        assert_eq!(cues, 3);
        assert!(ass.contains("Style: Title,Inter"));
        assert!(ass.contains("Style: Paragraph,Inter"));
        assert!(ass.contains("Style: Outro,Inter"));
        // Title cue concatenates "Chapter 3" + "\N" + "The Trust Stack".
        assert!(ass.contains("Chapter 3\\NThe Trust Stack"));
    }

    #[test]
    fn build_ass_falls_back_to_library_for_unknown_preset() {
        let mut spec = fixture("/tmp/out.mp4");
        spec.theme.preset = "totally-not-a-preset".into();
        let ass = build_ass(&spec, palette_for(&spec.theme.preset));
        // Shouldn't panic; should still emit a valid header.
        assert!(ass.starts_with("[Script Info]"));
        assert!(ass.contains("Style: Title,Inter"));
    }

    #[test]
    fn build_ass_skips_empty_paragraph_text() {
        let mut spec = fixture("/tmp/out.mp4");
        // Replace the paragraph scene's text with whitespace.
        if let Some(Scene::Paragraph { text, .. }) = spec.scenes.get_mut(1) {
            *text = "   ".into();
        }
        let ass = build_ass(&spec, palette_for("library"));
        // 2 scenes left (title + outro); blank paragraph dropped.
        assert_eq!(ass.matches("Dialogue: ").count(), 2);
    }

    #[test]
    fn build_filter_graph_image_background_includes_zoompan() {
        let spec = fixture("/tmp/out.mp4");
        let graph = build_filter_graph(&spec, Path::new("/tmp/x.ass"), Encoder::Software).unwrap();
        assert!(graph.starts_with("[0:v]"));
        assert!(graph.contains("zoompan="));
        assert!(graph.contains("subtitles="));
        assert!(graph.ends_with("[v_out]"));
    }

    #[test]
    fn build_filter_graph_color_background_skips_zoompan() {
        let mut spec = fixture("/tmp/out.mp4");
        spec.background = Background::Color {
            color: "#0F172A".into(),
        };
        let graph = build_filter_graph(&spec, Path::new("/tmp/x.ass"), Encoder::Software).unwrap();
        assert!(!graph.contains("zoompan="));
        assert!(graph.contains("subtitles="));
        assert!(graph.ends_with("[v_out]"));
    }

    #[test]
    fn build_filter_graph_disables_kenburns_when_flag_off() {
        let mut spec = fixture("/tmp/out.mp4");
        if let Background::Image { kenburns, .. } = &mut spec.background {
            *kenburns = false;
        }
        let graph = build_filter_graph(&spec, Path::new("/tmp/x.ass"), Encoder::Software).unwrap();
        assert!(!graph.contains("zoompan="));
        // Still scales to output dimensions.
        assert!(graph.contains("scale=1920:1080"));
    }

    #[test]
    fn build_filter_graph_escapes_ass_path() {
        let spec = fixture("/tmp/out.mp4");
        // Path with a colon — must be escaped for ffmpeg's filter parser.
        let graph =
            build_filter_graph(&spec, Path::new("/tmp/foo:bar.ass"), Encoder::Software).unwrap();
        assert!(graph.contains("/tmp/foo\\:bar.ass"));
    }

    #[test]
    fn build_filter_graph_appends_hwupload_for_vaapi() {
        // VAAPI moves frames to GPU memory after the libass composite.
        // The chain must end with `,format=nv12,hwupload[v_out]`.
        let spec = fixture("/tmp/out.mp4");
        let graph = build_filter_graph(&spec, Path::new("/tmp/x.ass"), Encoder::Vaapi).unwrap();
        assert!(
            graph.ends_with(",format=nv12,hwupload[v_out]"),
            "expected VAAPI hwupload tail, got: {graph}"
        );
    }

    #[test]
    fn build_filter_graph_no_hw_tail_for_software() {
        let spec = fixture("/tmp/out.mp4");
        let graph = build_filter_graph(&spec, Path::new("/tmp/x.ass"), Encoder::Software).unwrap();
        assert!(!graph.contains("hwupload"));
    }

    /// Real-ffmpeg smoke. Skipped unless run with `--ignored` because
    /// it shells out to ffmpeg; CI runners don't necessarily have it.
    /// Run locally with:
    ///   cargo test --bin listenai-api -- --ignored fast_path::tests::renders_a_real_mp4
    #[tokio::test]
    #[ignore]
    async fn renders_a_real_mp4() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("ch.wav");
        let out = dir.path().join("ch.video.mp4");

        // 4-second sine WAV via ffmpeg's lavfi source — we're just
        // looking for valid audio data the encoder won't reject.
        let st = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-ar",
                "24000",
                "-ac",
                "1",
            ])
            .arg(&wav)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("spawn ffmpeg for fixture");
        assert!(st.success(), "fixture wav generation failed");

        let spec = SceneSpec::new(
            ChapterMeta {
                number: 1,
                title: "Smoke".into(),
                duration_ms: 4_000,
            },
            AudioRef {
                wav: wav.clone(),
                peaks: None,
            },
            // Color background — keeps the test independent of any
            // sample cover image on disk.
            Background::Color {
                color: "#0F172A".into(),
            },
            Output::hd_1080(out.clone(), 24),
        )
        .with_theme(Theme {
            preset: "library".into(),
            primary: None,
            accent: None,
        })
        .push(Scene::Title {
            start_ms: 0,
            end_ms: 1_500,
            title: "Chapter 1".into(),
            subtitle: Some("Smoke".into()),
        })
        .push(Scene::Paragraph {
            start_ms: 1_500,
            end_ms: 3_000,
            text: "Quick brown fox.".into(),
            tile: None,
            highlight: "karaoke".into(),
            visual_kind: None,
            visual_params: None,
            manim_code: None,
        })
        .push(Scene::Outro {
            start_ms: 3_000,
            end_ms: 4_000,
            title: "Continue".into(),
            subtitle: None,
        });

        let (tx, mut rx) = mpsc::unbounded_channel();
        let drain = tokio::spawn(async move {
            let mut last = 0.0;
            while let Some(p) = rx.recv().await {
                last = p;
            }
            last
        });
        // Force software encoder so the smoke test doesn't require
        // a GPU on the runner.
        render(&spec, "ffmpeg", "none", "/dev/dri/renderD128", tx)
            .await
            .expect("render ok");
        let last_progress = drain.await.unwrap();
        assert!(last_progress > 0.0, "got zero progress events");

        assert!(out.exists(), "output mp4 missing");
        let metadata = std::fs::metadata(&out).unwrap();
        assert!(metadata.len() > 1_000, "output mp4 suspiciously small");
    }

    /// Real-VAAPI smoke. Skipped unless run with `--ignored` —
    /// requires `/dev/dri/renderD128` + a libva-backed encoder
    /// (Intel iGPU, AMD Radeon, virgl). Verifies the hwupload tail
    /// and the `-init_hw_device vaapi=...` device wiring.
    ///
    /// Run locally with:
    ///   cargo test --bin listenai-api -- --ignored fast_path::tests::renders_with_vaapi
    #[tokio::test]
    #[ignore]
    async fn renders_with_vaapi() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("ch.wav");
        let out = dir.path().join("ch.video.mp4");
        let st = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-ar",
                "24000",
                "-ac",
                "1",
            ])
            .arg(&wav)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("spawn ffmpeg for fixture");
        assert!(st.success());

        let spec = SceneSpec::new(
            ChapterMeta {
                number: 1,
                title: "VAAPI".into(),
                duration_ms: 4_000,
            },
            AudioRef { wav, peaks: None },
            Background::Color {
                color: "#0F172A".into(),
            },
            Output::hd_1080(out.clone(), 24),
        )
        .with_theme(Theme {
            preset: "library".into(),
            primary: None,
            accent: None,
        })
        .push(Scene::Title {
            start_ms: 0,
            end_ms: 2_000,
            title: "VAAPI".into(),
            subtitle: None,
        })
        .push(Scene::Outro {
            start_ms: 2_000,
            end_ms: 4_000,
            title: "Done".into(),
            subtitle: None,
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        // Use renderD129 by default since that's the integrated GPU
        // path on a hybrid Intel/NVIDIA dev box; override via
        // RENDER_VAAPI_DEVICE for boxes that expose it elsewhere.
        let device =
            std::env::var("RENDER_VAAPI_DEVICE").unwrap_or_else(|_| "/dev/dri/renderD129".into());
        render(&spec, "ffmpeg", "vaapi", &device, tx)
            .await
            .expect("render ok");
        assert!(out.exists());
        assert!(std::fs::metadata(&out).unwrap().len() > 1_000);
    }

    /// Real-NVENC smoke. Skipped unless run with `--ignored` because
    /// it requires a working NVIDIA GPU + driver. Useful for shaking
    /// out the NVENC arg list against a real driver before deploying.
    /// Run locally with:
    ///   cargo test --bin listenai-api -- --ignored fast_path::tests::renders_with_nvenc
    #[tokio::test]
    #[ignore]
    async fn renders_with_nvenc() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("ch.wav");
        let out = dir.path().join("ch.video.mp4");

        let st = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-ar",
                "24000",
                "-ac",
                "1",
            ])
            .arg(&wav)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("spawn ffmpeg for fixture");
        assert!(st.success(), "fixture wav generation failed");

        let spec = SceneSpec::new(
            ChapterMeta {
                number: 1,
                title: "NVENC".into(),
                duration_ms: 4_000,
            },
            AudioRef {
                wav: wav.clone(),
                peaks: None,
            },
            Background::Color {
                color: "#0F172A".into(),
            },
            Output::hd_1080(out.clone(), 24),
        )
        .with_theme(Theme {
            preset: "library".into(),
            primary: None,
            accent: None,
        })
        .push(Scene::Title {
            start_ms: 0,
            end_ms: 2_000,
            title: "NVENC".into(),
            subtitle: None,
        })
        .push(Scene::Outro {
            start_ms: 2_000,
            end_ms: 4_000,
            title: "Done".into(),
            subtitle: None,
        });

        let (tx, _rx) = mpsc::unbounded_channel();
        render(&spec, "ffmpeg", "nvenc", "/dev/dri/renderD128", tx)
            .await
            .expect("render ok");
        assert!(out.exists(), "output mp4 missing");
        let metadata = std::fs::metadata(&out).unwrap();
        assert!(metadata.len() > 1_000, "output mp4 suspiciously small");
    }
}
