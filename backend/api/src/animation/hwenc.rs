//! ffmpeg hardware encoder selection (Phase F.1f layer 1).
//!
//! The fast path (`fast_path::render`) used to hard-code
//! `libx264 -preset veryfast -crf 22`. On a host with a GPU that's
//! leaving 5–10× of encode speed on the table — NVENC and VAAPI both
//! comfortably hit 250–400 fps on 1080p H.264.
//!
//! This module adds an opt-in, host-detected hardware encoder picker:
//!
//!   * `LISTENAI_ANIMATE_HWENC=auto` (default) — probe `ffmpeg
//!     -encoders` + standard device files, pick the first match
//!     in priority NVENC > VAAPI > QSV, fall back to libx264.
//!   * `LISTENAI_ANIMATE_HWENC=nvenc|vaapi|qsv` — force the
//!     specified encoder; if it's not available the render will fail
//!     loudly at first attempt rather than silently falling back.
//!   * `LISTENAI_ANIMATE_HWENC=none` (or `software`, `cpu`) — force
//!     CPU libx264. Useful when a GPU host has flaky drivers and you
//!     want a known-good fallback while you triage.
//!
//! Detection is process-local + cached (one probe at first use, no
//! subsequent ffmpeg invocations). The same encoder choice is used
//! for every chapter for the lifetime of the process.

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    Software,
    Nvenc,
    Vaapi,
    Qsv,
}

impl Encoder {
    pub fn name(self) -> &'static str {
        match self {
            Encoder::Software => "libx264",
            Encoder::Nvenc => "h264_nvenc",
            Encoder::Vaapi => "h264_vaapi",
            Encoder::Qsv => "h264_qsv",
        }
    }
}

/// Default H.264 quality knob. Mirrored on the software path's
/// `-crf 22` and the GPU paths' equivalent quality args (`-cq` /
/// `-qp` / `-global_quality`). Lower = higher quality, larger file.
/// 22 is "broadcast pretty" — visually lossless for talking-head
/// content with karaoke text.
const DEFAULT_QUALITY: u8 = 22;

/// Resolve the configured override into an [`Encoder`]. Performs
/// detection only when `override_choice` is empty or `auto`. Logs
/// the chosen encoder so deployments can verify their GPU setup
/// without enabling debug logs.
///
/// `vaapi_device` is the absolute DRI render-node path to probe for
/// VAAPI/QSV (typically `/dev/dri/renderD128`, but `…/renderD129` on
/// hybrid hosts). Used both at probe time and inside
/// `pre_input_args` so the encode init points at the same node we
/// detected.
pub async fn detect(ffmpeg_bin: &str, override_choice: &str, vaapi_device: &str) -> Encoder {
    match override_choice.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => {} // fall through to autodetect
        "none" | "software" | "cpu" => {
            info!("hwenc: forced software (libx264) by config");
            return Encoder::Software;
        }
        "nvenc" => {
            info!("hwenc: forced NVENC by config");
            return Encoder::Nvenc;
        }
        "vaapi" => {
            info!("hwenc: forced VAAPI by config");
            return Encoder::Vaapi;
        }
        "qsv" => {
            info!("hwenc: forced QSV by config");
            return Encoder::Qsv;
        }
        other => {
            warn!(
                value = other,
                "hwenc: unknown LISTENAI_ANIMATE_HWENC value; falling back to software"
            );
            return Encoder::Software;
        }
    }

    let bin = if ffmpeg_bin.trim().is_empty() {
        "ffmpeg"
    } else {
        ffmpeg_bin
    };
    let encoders = list_encoders(bin).await.unwrap_or_default();

    let pick = pick_from(&encoders, &SystemProbe::live(vaapi_device));
    log_pick(pick);
    pick
}

/// Pure-function half of `detect`: given the ffmpeg `-encoders` text
/// and a system-probe abstraction, return the best encoder. Lets the
/// detection logic be unit-tested without shelling out.
fn pick_from(encoders_text: &str, probe: &SystemProbe) -> Encoder {
    if encoders_text.contains("h264_nvenc") && probe.has_nvidia {
        return Encoder::Nvenc;
    }
    if encoders_text.contains("h264_vaapi") && probe.has_dri_render {
        return Encoder::Vaapi;
    }
    if encoders_text.contains("h264_qsv") && probe.has_dri_render {
        return Encoder::Qsv;
    }
    Encoder::Software
}

fn log_pick(pick: Encoder) {
    match pick {
        Encoder::Software => info!(
            encoder = pick.name(),
            "hwenc: no GPU encoder detected, using software libx264"
        ),
        Encoder::Nvenc => info!(
            encoder = pick.name(),
            "hwenc: detected NVENC + nvidia device"
        ),
        Encoder::Vaapi => info!(
            encoder = pick.name(),
            "hwenc: detected VAAPI + DRI render node"
        ),
        Encoder::Qsv => info!(
            encoder = pick.name(),
            "hwenc: detected QSV + DRI render node"
        ),
    }
}

/// Per-encoder ffmpeg argv tail. Caller appends these after `-i` and
/// before the output path. Quality numbers are mapped to each
/// encoder's preferred knob: `-crf` for x264, `-cq` for NVENC,
/// `-qp` for VAAPI, `-global_quality` for QSV.
pub fn encoder_args(encoder: Encoder) -> Vec<String> {
    encoder_args_with_quality(encoder, DEFAULT_QUALITY)
}

fn encoder_args_with_quality(encoder: Encoder, quality: u8) -> Vec<String> {
    let q = quality.to_string();
    match encoder {
        Encoder::Software => vec![
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            "veryfast".into(),
            "-crf".into(),
            q,
        ],
        Encoder::Nvenc => vec![
            "-c:v".into(),
            "h264_nvenc".into(),
            "-preset".into(),
            "p4".into(),
            "-rc:v".into(),
            "vbr".into(),
            "-cq".into(),
            q,
            "-b:v".into(),
            "0".into(),
        ],
        Encoder::Vaapi => vec!["-c:v".into(), "h264_vaapi".into(), "-qp".into(), q],
        Encoder::Qsv => vec![
            "-c:v".into(),
            "h264_qsv".into(),
            "-preset".into(),
            "veryfast".into(),
            "-global_quality".into(),
            q,
        ],
    }
}

/// VAAPI-specific extra args that go BEFORE `-i` to bind the GPU
/// device for the encode pass. Empty for every other encoder. Caller
/// appends to the very front of the ffmpeg argv (after `-y`).
///
/// `vaapi_device` should be the same DRI render-node path we
/// detected against — defaults to `/dev/dri/renderD128`.
pub fn pre_input_args(encoder: Encoder, vaapi_device: &str) -> Vec<String> {
    match encoder {
        Encoder::Vaapi => vec![
            "-init_hw_device".into(),
            format!("vaapi=va:{vaapi_device}"),
            "-filter_hw_device".into(),
            "va".into(),
        ],
        _ => Vec::new(),
    }
}

/// VAAPI-specific filter-graph tail. Appended before the `[v_out]`
/// label so the rasterized frame gets uploaded to GPU memory before
/// the encoder consumes it. libass + `subtitles` filter both run on
/// CPU pixel data, so the upload has to happen *after* the text
/// composite, not before.
pub fn filter_graph_tail(encoder: Encoder) -> &'static str {
    match encoder {
        Encoder::Vaapi => ",format=nv12,hwupload",
        _ => "",
    }
}

async fn list_encoders(ffmpeg_bin: &str) -> Option<String> {
    let output = Command::new(ffmpeg_bin)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Snapshot of host capabilities we sniff during detection. Owns no
/// I/O — `live()` populates it once from `std::fs::Path::exists`
/// calls; tests construct it inline.
#[derive(Debug, Clone, Copy)]
struct SystemProbe {
    has_nvidia: bool,
    has_dri_render: bool,
}

impl SystemProbe {
    fn live(vaapi_device: &str) -> Self {
        Self {
            // `/dev/nvidiactl` exists whenever the proprietary driver
            // is loaded; sufficient signal for "NVENC will work."
            has_nvidia: Path::new("/dev/nvidiactl").exists(),
            // The configured DRI render node — same path we'll hand
            // to `-init_hw_device` later, so detection and encode
            // can't disagree about which GPU we're targeting.
            has_dri_render: Path::new(vaapi_device).exists(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_DEVICE: &str = "/dev/dri/renderD128";

    #[test]
    fn override_software_aliases() {
        // `none` / `software` / `cpu` all mean libx264.
        for v in ["none", "software", "cpu", "NONE", "Software"] {
            // detect short-circuits before any I/O when override is set,
            // so we can call it from a sync test via tokio's runtime.
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            let got = rt.block_on(detect("ffmpeg-not-real", v, TEST_DEVICE));
            assert_eq!(got, Encoder::Software, "override {v}");
        }
    }

    #[test]
    fn override_explicit_encoders() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert_eq!(
            rt.block_on(detect("ffmpeg-not-real", "nvenc", TEST_DEVICE)),
            Encoder::Nvenc
        );
        assert_eq!(
            rt.block_on(detect("ffmpeg-not-real", "vaapi", TEST_DEVICE)),
            Encoder::Vaapi
        );
        assert_eq!(
            rt.block_on(detect("ffmpeg-not-real", "qsv", TEST_DEVICE)),
            Encoder::Qsv
        );
    }

    #[test]
    fn override_unknown_falls_back_to_software() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert_eq!(
            rt.block_on(detect(
                "ffmpeg-not-real",
                "magic-future-encoder",
                TEST_DEVICE
            )),
            Encoder::Software
        );
    }

    #[test]
    fn pick_prefers_nvenc_when_present() {
        let encoders = "V..... h264_nvenc\nV..... h264_vaapi\nV..... libx264\n";
        let probe = SystemProbe {
            has_nvidia: true,
            has_dri_render: true,
        };
        assert_eq!(pick_from(encoders, &probe), Encoder::Nvenc);
    }

    #[test]
    fn pick_falls_through_to_vaapi_when_no_nvidia_device() {
        // ffmpeg is built with NVENC but the box has no nvidia
        // driver loaded — picker shouldn't hand us an encoder that'll
        // fail at runtime.
        let encoders = "V..... h264_nvenc\nV..... h264_vaapi\nV..... libx264\n";
        let probe = SystemProbe {
            has_nvidia: false,
            has_dri_render: true,
        };
        assert_eq!(pick_from(encoders, &probe), Encoder::Vaapi);
    }

    #[test]
    fn pick_falls_through_to_qsv_when_only_qsv_listed() {
        let encoders = "V..... h264_qsv\nV..... libx264\n";
        let probe = SystemProbe {
            has_nvidia: false,
            has_dri_render: true,
        };
        assert_eq!(pick_from(encoders, &probe), Encoder::Qsv);
    }

    #[test]
    fn pick_falls_back_to_software_when_no_devices() {
        let encoders = "V..... h264_nvenc\nV..... h264_vaapi\nV..... h264_qsv\nV..... libx264\n";
        let probe = SystemProbe {
            has_nvidia: false,
            has_dri_render: false,
        };
        assert_eq!(pick_from(encoders, &probe), Encoder::Software);
    }

    #[test]
    fn pick_falls_back_to_software_when_ffmpeg_lacks_hw_encoders() {
        // A minimal ffmpeg build (debian-stable's, sometimes) has
        // libx264 only — picker must detect and fall back even with
        // a GPU on the host.
        let encoders = "V..... libx264\n";
        let probe = SystemProbe {
            has_nvidia: true,
            has_dri_render: true,
        };
        assert_eq!(pick_from(encoders, &probe), Encoder::Software);
    }

    #[test]
    fn encoder_args_first_arg_is_codec_name() {
        for enc in [
            Encoder::Software,
            Encoder::Nvenc,
            Encoder::Vaapi,
            Encoder::Qsv,
        ] {
            let args = encoder_args(enc);
            assert_eq!(args[0], "-c:v");
            assert_eq!(args[1], enc.name());
        }
    }

    #[test]
    fn encoder_args_quality_lands_in_correct_arg() {
        // The quality knob differs per encoder; verify it flows into
        // the right one for each.
        let q = 18u8;
        let sw = encoder_args_with_quality(Encoder::Software, q);
        let sw_idx = sw.iter().position(|s| s == "-crf").unwrap();
        assert_eq!(sw[sw_idx + 1], "18");

        let nv = encoder_args_with_quality(Encoder::Nvenc, q);
        let nv_idx = nv.iter().position(|s| s == "-cq").unwrap();
        assert_eq!(nv[nv_idx + 1], "18");

        let va = encoder_args_with_quality(Encoder::Vaapi, q);
        let va_idx = va.iter().position(|s| s == "-qp").unwrap();
        assert_eq!(va[va_idx + 1], "18");

        let qs = encoder_args_with_quality(Encoder::Qsv, q);
        let qs_idx = qs.iter().position(|s| s == "-global_quality").unwrap();
        assert_eq!(qs[qs_idx + 1], "18");
    }

    #[test]
    fn pre_input_args_only_set_for_vaapi() {
        assert!(pre_input_args(Encoder::Software, TEST_DEVICE).is_empty());
        assert!(pre_input_args(Encoder::Nvenc, TEST_DEVICE).is_empty());
        assert!(pre_input_args(Encoder::Qsv, TEST_DEVICE).is_empty());

        let va = pre_input_args(Encoder::Vaapi, TEST_DEVICE);
        assert!(va.contains(&"-init_hw_device".to_string()));
        assert!(va.iter().any(|s| s.contains("/dev/dri/renderD128")));
    }

    #[test]
    fn pre_input_args_uses_configured_vaapi_device() {
        // Override device path lands in the -init_hw_device value.
        let va = pre_input_args(Encoder::Vaapi, "/dev/dri/renderD129");
        assert!(va.iter().any(|s| s == "vaapi=va:/dev/dri/renderD129"));
    }

    #[test]
    fn filter_graph_tail_only_set_for_vaapi() {
        assert_eq!(filter_graph_tail(Encoder::Software), "");
        assert_eq!(filter_graph_tail(Encoder::Nvenc), "");
        assert_eq!(filter_graph_tail(Encoder::Qsv), "");
        assert_eq!(filter_graph_tail(Encoder::Vaapi), ",format=nv12,hwupload");
    }
}
