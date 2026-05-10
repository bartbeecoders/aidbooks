//! Songbook snippet job: download N evenly-spaced clips of the song
//! the user picked, store them under
//! `<storage>/<audiobook>/snippets/snippet-<i>.wav`, and let the
//! YouTube publisher splice them between chapters at upload time.
//!
//! End-to-end flow:
//! 1. Tinyfish search for `"<topic> official audio youtube"` → first
//!    youtube.com/watch URL.
//! 2. `yt-dlp --print duration <url>` → song length in seconds.
//! 3. Pick N evenly-spaced start offsets inside `[10 %, 90 %]` of the
//!    song so we skip the intro/outro silence and any "thank you for
//!    listening" tails the LLM analysis can't unpack.
//! 4. For each offset, run yt-dlp with `--download-sections` so only
//!    that slice gets fetched, normalised to mono PCM at the TTS
//!    sample rate. Output lands at `snippet-<i>.wav`.
//!
//! Best-effort throughout: a missing yt-dlp, an empty Tinyfish key, a
//! 5xx, an age-restricted video — every failure is a `warn!` and the
//! audiobook keeps its outline + chapters; only the audio clips are
//! skipped. The publisher tolerates a partial set: it splices
//! whichever snippets exist on disk and ignores gaps.
//!
//! Per-clip duration is fixed at `SNIPPET_SECONDS`. Tweak there before
//! reaching for a config knob — N is already user-controlled.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use listenai_core::{Error, Result};
use tokio::process::Command;
use tracing::{info, warn};

use crate::generation::songbook;
use crate::state::AppState;

/// Length of every snippet clip. 12 s lands in the YouTube
/// "non-trivial transformative use" zone for music quotation while
/// still giving listeners enough to recognise the moment.
const SNIPPET_SECONDS: f64 = 12.0;

/// Fade-in/out applied to every snippet during normalization so the
/// transition between narration and music isn't a hard cut. 0.5 s
/// is enough to round the edge audibly without eating into the
/// recognisable middle of the clip.
const FADE_SECONDS: f64 = 0.5;

/// Per-yt-dlp-call wall clock cap. Sized for one snippet download on
/// a slow connection; the duration probe is a separate call with the
/// same cap. The job spawns N+1 calls sequentially, so the total job
/// budget is roughly `(N+1) × YT_DLP_CALL_TIMEOUT`.
const YT_DLP_CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Absolute directory snippets land in. The publisher reads from
/// here; the job writes to here. One per audiobook so re-runs
/// overwrite cleanly.
pub fn snippet_dir(state: &AppState, audiobook_id: &str) -> PathBuf {
    state
        .config()
        .storage_path
        .join(audiobook_id)
        .join("snippets")
}

/// Read a WAV file's duration in milliseconds without spawning
/// ffprobe. Walks the RIFF chunks, pulls `byte_rate` from the
/// `fmt ` chunk and the audio body size from the `data` chunk:
/// `duration_ms = data_size * 1000 / byte_rate`. Used by the
/// publisher (to keep image-segment timing in sync with the
/// spliced WAVs) and by the preview endpoint (to surface the
/// per-clip duration to the frontend).
pub fn wav_duration_ms(path: &Path) -> std::io::Result<u64> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    let mut f = File::open(path)?;
    let mut header = [0u8; 12];
    f.read_exact(&mut header)?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a RIFF/WAVE file",
        ));
    }

    let mut byte_rate: Option<u32> = None;
    let mut data_size: Option<u32> = None;
    loop {
        let mut chunk_hdr = [0u8; 8];
        if f.read_exact(&mut chunk_hdr).is_err() {
            break;
        }
        let id = &chunk_hdr[0..4];
        let size = u32::from_le_bytes([chunk_hdr[4], chunk_hdr[5], chunk_hdr[6], chunk_hdr[7]]);
        if id == b"fmt " {
            // fmt chunk: format(2) channels(2) sample_rate(4) byte_rate(4)
            // block_align(2) bits_per_sample(2). We only need byte_rate.
            let mut buf = vec![0u8; size as usize];
            f.read_exact(&mut buf)?;
            if buf.len() >= 12 {
                byte_rate = Some(u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]));
            }
        } else if id == b"data" {
            data_size = Some(size);
            break;
        } else {
            // Skip unknown chunk (LIST, INFO, JUNK, …). Sizes are
            // padded to even bytes per RIFF spec.
            let pad = size & 1;
            f.seek(SeekFrom::Current((size + pad) as i64))?;
        }
    }

    let br = byte_rate.unwrap_or(0).max(1);
    let ds = data_size.unwrap_or(0);
    Ok((ds as u64 * 1000) / br as u64)
}

/// Outcome of `download_into` — what the caller needs to render UI
/// or splice audio. `youtube_url` is `None` only when the lookup
/// itself failed (e.g. missing Tinyfish key); `error` is set when
/// the *whole* operation hit a structural problem (yt-dlp missing,
/// duration probe failed, song too short). Per-clip download
/// failures are logged but don't surface here — callers infer them
/// from `produced.len() < count`.
#[derive(Debug, Default)]
pub struct DownloadOutcome {
    pub youtube_url: Option<String>,
    pub produced: Vec<u32>,
    pub error: Option<String>,
}

/// Run the snippet job for an already-persisted audiobook.
///
/// Caller has already confirmed `is_songbook && snippet_count > 0`;
/// this entrypoint just executes. On hard failures (Tinyfish blew
/// up, yt-dlp missing, no YouTube URL found) it logs a warn and
/// returns `Ok(())` — the publisher will see no snippets on disk
/// and skip the splice step. We only return `Err` for genuinely
/// unexpected I/O errors creating the storage directory.
pub async fn run(state: &AppState, audiobook_id: &str, topic: &str, count: u32) -> Result<()> {
    let dir = snippet_dir(state, audiobook_id);
    let outcome = download_into(state, &dir, topic, count).await?;
    info!(
        audiobook_id,
        ok = outcome.produced.len(),
        requested = count,
        ?outcome.error,
        "song_snippets: job complete"
    );
    Ok(())
}

/// Download up to `count` evenly-spaced snippets of the song
/// referenced by `topic` into `dir`. Shared between the audiobook
/// snippet job and the preview endpoint. Best-effort: every
/// upstream failure (no Tinyfish key, no YouTube URL, missing
/// yt-dlp, song too short, age-gated, …) becomes an `outcome.error`
/// string and a `warn!` log; the function only returns `Err` for
/// I/O errors creating `dir`.
pub async fn download_into(
    state: &AppState,
    dir: &Path,
    topic: &str,
    count: u32,
) -> Result<DownloadOutcome> {
    let mut out = DownloadOutcome::default();
    if count == 0 {
        return Ok(out);
    }
    let count = count.min(12);
    let cfg = state.config();

    let yt_dlp = cfg.yt_dlp_bin.trim().to_string();
    if yt_dlp.is_empty() {
        let msg = "yt_dlp_bin is empty — set LISTENAI_YT_DLP_BIN".to_string();
        warn!("song_snippets: {msg}");
        out.error = Some(msg);
        return Ok(out);
    }

    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("create snippet dir {}: {e}", dir.display())))?;

    let url = match songbook::find_youtube_url(state, topic).await {
        Some(u) => u,
        None => {
            let msg = "no YouTube URL found via Tinyfish (check LISTENAI_TINYFISH_API_KEY \
                       and that the topic is recognisable as a song)"
                .to_string();
            warn!(topic, "song_snippets: {msg}");
            out.error = Some(msg);
            return Ok(out);
        }
    };
    out.youtube_url = Some(url.clone());

    let total = match probe_duration(&yt_dlp, &url).await {
        Ok(t) if t > SNIPPET_SECONDS * 2.0 => t,
        Ok(t) => {
            let msg = format!("song is only {t:.1}s — too short for a {SNIPPET_SECONDS}s clip");
            warn!("song_snippets: {msg}");
            out.error = Some(msg);
            return Ok(out);
        }
        Err(e) => {
            let msg = format!("yt-dlp duration probe failed: {e}");
            warn!("song_snippets: {msg}");
            out.error = Some(msg);
            return Ok(out);
        }
    };

    let offsets = evenly_spaced_offsets(total, count);
    let sr = cfg.xai_sample_rate_hz;
    let ffmpeg = cfg.ffmpeg_bin.trim().to_string();
    if ffmpeg.is_empty() {
        let msg = "ffmpeg_bin is empty — cannot normalize snippet WAVs".to_string();
        warn!("song_snippets: {msg}");
        out.error = Some(msg);
        return Ok(out);
    }
    for (i, start) in offsets.iter().enumerate() {
        let idx = (i as u32) + 1;
        let path = dir.join(format!("snippet-{idx}.wav"));
        if let Err(e) = download_clip(&yt_dlp, &url, &path, *start, sr).await {
            warn!(
                snippet = idx,
                error = %e,
                "song_snippets: clip download failed"
            );
            continue;
        }
        // Lock the WAV to exactly the TTS format (PCM s16le, mono,
        // matching sample rate). yt-dlp's `--postprocessor-args`
        // can produce float-PCM or different bit depths depending
        // on its bundled ffmpeg version, and the publisher's audio
        // concat demuxer interprets every file as the *first*
        // stream's format → mismatched snippets surface as muted or
        // scrambled bytes in the published video.
        if let Err(e) = normalize_wav(&ffmpeg, &path, sr).await {
            warn!(
                snippet = idx,
                error = %e,
                "song_snippets: normalize failed; dropping clip"
            );
            let _ = tokio::fs::remove_file(&path).await;
            continue;
        }
        out.produced.push(idx);
    }
    if out.produced.is_empty() && out.error.is_none() {
        out.error = Some("yt-dlp ran but produced no clips (see backend logs)".to_string());
    }
    Ok(out)
}

/// Spread `count` start offsets evenly across `[10 %, 90 %]` of the
/// song so we skip intro silence + outro tails. Returns offsets in
/// seconds.
fn evenly_spaced_offsets(total: f64, count: u32) -> Vec<f64> {
    let n = count.max(1) as f64;
    let lo = total * 0.10;
    let hi = (total - SNIPPET_SECONDS).min(total * 0.90);
    if hi <= lo {
        // Pathological short song: just clamp to a single window.
        return vec![lo.max(0.0)];
    }
    if count == 1 {
        return vec![lo + (hi - lo) * 0.5];
    }
    (0..count)
        .map(|i| lo + (hi - lo) * (i as f64) / (n - 1.0))
        .collect()
}

/// `yt-dlp --no-download --print duration <url>` → song length in
/// seconds.
async fn probe_duration(yt_dlp: &str, url: &str) -> std::result::Result<f64, String> {
    let bytes = run_yt_dlp(
        yt_dlp,
        &[
            "--no-download",
            "--no-warnings",
            "--quiet",
            "--print",
            "duration",
            url,
        ],
    )
    .await?;
    let s = String::from_utf8_lossy(&bytes).trim().to_string();
    s.parse::<f64>()
        .map_err(|e| format!("parse duration `{s}`: {e}"))
}

/// Download a single `[start .. start + SNIPPET_SECONDS]` slice as a
/// mono WAV at `sample_rate_hz`. Uses yt-dlp's `--download-sections`
/// so only that slice is fetched.
async fn download_clip(
    yt_dlp: &str,
    url: &str,
    out_path: &Path,
    start: f64,
    sample_rate_hz: u32,
) -> std::result::Result<(), String> {
    // yt-dlp's `-o` template appends the right extension based on
    // `--audio-format`, so we hand it the path *without* `.wav` and
    // let yt-dlp tack it on. The existing extension is stripped to
    // avoid producing `snippet-1.wav.wav`.
    let stem = out_path
        .with_extension("")
        .into_os_string()
        .into_string()
        .map_err(|p| format!("non-utf8 snippet path: {p:?}"))?;
    let template = format!("{stem}.%(ext)s");
    let section = format!("*{:.2}-{:.2}", start, start + SNIPPET_SECONDS);
    let pp_args = format!("ffmpeg_o:-ar {sample_rate_hz} -ac 1");
    run_yt_dlp(
        yt_dlp,
        &[
            "--quiet",
            "--no-warnings",
            "--no-playlist",
            "--no-progress",
            "--download-sections",
            &section,
            "--force-keyframes-at-cuts",
            "-x",
            "--audio-format",
            "wav",
            "--postprocessor-args",
            &pp_args,
            "-o",
            &template,
            url,
        ],
    )
    .await?;

    if !out_path.exists() {
        return Err(format!(
            "yt-dlp finished but output missing: {}",
            out_path.display()
        ));
    }
    Ok(())
}

/// Re-encode `path` in place to the publisher-friendly format:
/// PCM s16le, mono, `sample_rate_hz`. Writes to a sibling
/// `*.norm.wav` and renames over the original on success.
///
/// This shouldn't be necessary in theory — yt-dlp's
/// `--postprocessor-args "ffmpeg_o:-ar … -ac 1"` already requests
/// the right rate + channel count. In practice the bundled ffmpeg
/// can default to `pcm_f32le` or skip the sample-format change, and
/// our publisher's audio `concat` demuxer assumes every file
/// matches the first stream's format byte-for-byte. Locking sample
/// format + bit depth here is the cheap insurance.
async fn normalize_wav(
    ffmpeg: &str,
    path: &Path,
    sample_rate_hz: u32,
) -> std::result::Result<(), String> {
    let tmp = path.with_extension("norm.wav");
    // Fade in from 0 → FADE_SECONDS, fade out over the last
    // FADE_SECONDS of the clip. Snippet length is fixed by the
    // caller (`SNIPPET_SECONDS`), so we can hard-code the start of
    // the out-fade. If yt-dlp delivered a shorter clip the fade-out
    // start runs past the end and ffmpeg silently no-ops it — the
    // worst case is "no fade-out", not garbage audio.
    let fade_filter = format!(
        "afade=t=in:st=0:d={fade},afade=t=out:st={out_start}:d={fade}",
        fade = FADE_SECONDS,
        out_start = SNIPPET_SECONDS - FADE_SECONDS,
    );
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-af")
        .arg(&fade_filter)
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg(sample_rate_hz.to_string())
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg("-sample_fmt")
        .arg("s16")
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = tokio::time::timeout(YT_DLP_CALL_TIMEOUT, cmd.output())
        .await
        .map_err(|_| format!("ffmpeg normalize timeout after {YT_DLP_CALL_TIMEOUT:?}"))?
        .map_err(|e| format!("spawn `{ffmpeg}`: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!(
            "ffmpeg exit={}: {}",
            out.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| format!("rename normalized wav: {e}"))?;
    Ok(())
}

/// Spawn `yt-dlp <args>` capped by `YT_DLP_CALL_TIMEOUT`. Returns
/// stdout bytes on success.
async fn run_yt_dlp(bin: &str, args: &[&str]) -> std::result::Result<Vec<u8>, String> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let fut = cmd.output();
    let out = tokio::time::timeout(YT_DLP_CALL_TIMEOUT, fut)
        .await
        .map_err(|_| format!("timeout after {:?}", YT_DLP_CALL_TIMEOUT))?
        .map_err(|e| format!("spawn `{bin}`: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "exit={}: {}",
            out.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_evenly_spaced_within_window() {
        let total = 200.0;
        let xs = evenly_spaced_offsets(total, 3);
        assert_eq!(xs.len(), 3);
        assert!(xs[0] >= total * 0.10 - 0.01);
        assert!(xs[2] <= (total - SNIPPET_SECONDS) + 0.01);
        assert!(xs[2] <= total * 0.90 + 0.01);
        // Strictly increasing.
        assert!(xs[1] > xs[0]);
        assert!(xs[2] > xs[1]);
    }

    #[test]
    fn offsets_short_song_falls_back() {
        // 20 s song; SNIPPET_SECONDS=12 leaves only 8 s window which
        // is < 10 %–90 % spread.
        let xs = evenly_spaced_offsets(20.0, 3);
        assert_eq!(xs.len(), 1);
    }
}
