//! Audio persistence + waveform peak computation.
//!
//! Layout under `Config.storage_path`:
//!   <storage_path>/<audiobook_id>/ch-<n>.wav
//!   <storage_path>/<audiobook_id>/ch-<n>.waveform.json
//!
//! WAV rather than Opus for Phase 4 — a pure-Rust path (hound) keeps the
//! dependency graph simple. Opus + M4B land in a polish pass.

use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use hound::{SampleFormat, WavSpec, WavWriter};
use listenai_core::{Error, Result};
use serde::Serialize;

pub const WAVEFORM_BUCKETS: usize = 500;

pub struct ChapterAudioFiles {
    pub wav_path: PathBuf,
    /// Sibling JSON file path. Kept in the struct even though the
    /// audio-generation layer only needs the WAV path — callers can use
    /// this directly if they want to inspect/clean up the waveform file.
    #[allow(dead_code)]
    pub waveform_path: PathBuf,
    pub duration_ms: u64,
    pub bytes: u64,
}

/// Write the given PCM samples to WAV and compute + persist a waveform
/// peaks file alongside it. Overwrites any previous files for this chapter.
///
/// Layout: `<storage>/<audiobook>/<language>/ch-<n>.wav`. Per-language
/// subdirs keep parallel narrations from clobbering each other.
pub fn write_chapter(
    storage_root: &Path,
    audiobook_id: &str,
    chapter_number: u32,
    language: &str,
    samples: &[i16],
    sample_rate_hz: u32,
) -> Result<ChapterAudioFiles> {
    let dir = storage_root.join(audiobook_id).join(language);
    fs::create_dir_all(&dir)
        .map_err(|e| Error::Other(anyhow::anyhow!("create audio dir {dir:?}: {e}")))?;

    let wav_path = dir.join(format!("ch-{chapter_number}.wav"));
    let waveform_path = dir.join(format!("ch-{chapter_number}.waveform.json"));

    let spec = WavSpec {
        channels: 1,
        sample_rate: sample_rate_hz,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let writer = BufWriter::new(
        File::create(&wav_path).map_err(|e| Error::Other(anyhow::anyhow!("create wav: {e}")))?,
    );
    let mut w = WavWriter::new(writer, spec)
        .map_err(|e| Error::Other(anyhow::anyhow!("wav header: {e}")))?;
    for s in samples {
        w.write_sample(*s)
            .map_err(|e| Error::Other(anyhow::anyhow!("wav write sample: {e}")))?;
    }
    w.finalize()
        .map_err(|e| Error::Other(anyhow::anyhow!("wav finalize: {e}")))?;

    let peaks = compute_peaks(samples, WAVEFORM_BUCKETS);
    let waveform = Waveform {
        sample_rate_hz,
        buckets: peaks.len() as u32,
        peaks,
    };
    let mut f = File::create(&waveform_path)
        .map_err(|e| Error::Other(anyhow::anyhow!("create waveform: {e}")))?;
    serde_json::to_writer(&mut f, &waveform)
        .map_err(|e| Error::Other(anyhow::anyhow!("encode waveform: {e}")))?;
    f.flush()
        .map_err(|e| Error::Other(anyhow::anyhow!("flush waveform: {e}")))?;

    let duration_ms = if sample_rate_hz == 0 {
        0
    } else {
        (samples.len() as u64 * 1000) / sample_rate_hz as u64
    };
    let bytes = fs::metadata(&wav_path).map(|m| m.len()).unwrap_or_default();

    Ok(ChapterAudioFiles {
        wav_path,
        waveform_path,
        duration_ms,
        bytes,
    })
}

#[derive(Debug, Serialize)]
struct Waveform {
    sample_rate_hz: u32,
    buckets: u32,
    peaks: Vec<f32>,
}

/// Break the samples into `target_buckets` windows and return the max-abs
/// magnitude in each, normalised to `[0.0, 1.0]`. Short clips return fewer
/// buckets rather than fake interpolation.
fn compute_peaks(samples: &[i16], target_buckets: usize) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let buckets = target_buckets.min(samples.len());
    let per = samples.len().div_ceil(buckets);
    let scale = i16::MAX as f32;
    (0..buckets)
        .map(|b| {
            let start = b * per;
            let end = (start + per).min(samples.len());
            let mut peak: i16 = 0;
            for s in &samples[start..end] {
                let mag = s.unsigned_abs() as i16;
                if mag > peak {
                    peak = mag;
                }
            }
            peak as f32 / scale
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_are_bounded_between_zero_and_one() {
        let samples = vec![1000_i16; 24_000];
        let peaks = compute_peaks(&samples, 100);
        assert_eq!(peaks.len(), 100);
        for p in peaks {
            assert!((0.0..=1.0).contains(&p));
        }
    }

    #[test]
    fn silence_makes_zero_peaks() {
        let samples = vec![0_i16; 24_000];
        let peaks = compute_peaks(&samples, 50);
        assert!(peaks.iter().all(|p| *p == 0.0));
    }

    #[test]
    fn fewer_buckets_when_short() {
        let samples = vec![100_i16; 10];
        let peaks = compute_peaks(&samples, 500);
        assert_eq!(peaks.len(), 10);
    }

    #[test]
    fn wav_round_trip() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let samples: Vec<i16> = (0..24_000).map(|i| (i as i16).wrapping_mul(3)).collect();
        let files = write_chapter(dir.path(), "abc", 1, "en", &samples, 24_000).unwrap();
        assert!(files.wav_path.exists());
        assert!(files.waveform_path.exists());
        assert_eq!(files.duration_ms, 1000);
        // Read back and check it parses as a WAV.
        let reader = hound::WavReader::open(&files.wav_path).unwrap();
        assert_eq!(reader.spec().sample_rate, 24_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.len(), 24_000);
    }
}
