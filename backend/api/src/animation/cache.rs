//! Skip-when-unchanged cache for the chapter renderer.
//!
//! The animate publisher renders the same chapter the same way every time
//! its inputs are stable: same body text, same cover, same tiles, same
//! theme, same fps. A SHA-256 over the (canonicalized) `SceneSpec` plus
//! the mtimes of every referenced asset captures all of that.
//!
//! On render entry: if `<mp4>.hash` exists, the MP4 exists, and the hash
//! matches what we'd compute now, we skip the render and reuse the file.
//! On successful render: write `<mp4>.hash` next to the MP4. The hash
//! file is small (~64 bytes) and lives alongside the artefact it
//! describes, so GC of the MP4 takes the hash file with it.
//!
//! Note: this is *renderer-path-aware*. Different render paths (Revideo
//! vs the planned ffmpeg fast path) are not byte-for-byte identical, so
//! the hash incorporates a `render_path` label — flipping the fast-path
//! flag invalidates the cache cleanly without breaking anyone.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

use super::spec::{Background, Scene, SceneSpec};

/// Render-path labels folded into the hash. Bump these whenever the
/// pipeline changes in a way that affects pixel output but doesn't
/// show up in the spec (Chromium upgrade, Revideo upgrade, ffmpeg
/// filter graph change). Three paths exist today; the publisher
/// picks the right label based on `animate_fast_path`,
/// `animate_mock`, and effective STEM-ness.
pub const REVIDEO_PATH_LABEL: &str = "revideo-v1";
pub const FFMPEG_PATH_LABEL: &str = "ffmpeg-v1";
/// Phase G.6 — per-segment fast-path + Manim diagrams. Different
/// pixel output from the plain fast path because diagram windows
/// render through Manim, so the cache *must* invalidate when the
/// user toggles STEM.
pub const FFMPEG_STEM_PATH_LABEL: &str = "ffmpeg-stem-v1";

/// Returns the path to the cache sidecar file for a given output MP4.
pub fn cache_path(mp4: &Path) -> PathBuf {
    let mut p = mp4.to_path_buf();
    let new_name = match mp4.file_name().and_then(|s| s.to_str()) {
        Some(name) => format!("{name}.hash"),
        None => "video.mp4.hash".to_string(),
    };
    p.set_file_name(new_name);
    p
}

/// Compute the hex-encoded SHA-256 of the canonicalized spec + the
/// mtimes of every referenced input file + the active render-path
/// label (so flipping `animate_fast_path` invalidates the cache
/// cleanly). Stable across runs as long as the inputs are stable;
/// differs if any tile, the cover, the audio, the spec itself, or
/// the render path changes.
pub fn compute_spec_hash(spec: &SceneSpec, render_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(render_path.as_bytes());
    hasher.update(b"\0");

    // Serialize a copy of the spec with the absolute output path
    // emptied — the path itself is metadata about *where* we render,
    // not *what* we render. Two builds that point at different output
    // dirs but render the same content should hit the cache.
    let mut canonical = spec.clone();
    canonical.output.mp4 = PathBuf::new();
    let json = serde_json::to_string(&canonical).unwrap_or_default();
    hasher.update(json.as_bytes());
    hasher.update(b"\0");

    // Fold mtimes of every referenced input file. Missing/unreadable
    // files contribute a sentinel — render will fail later, but the
    // cache check shouldn't return a stale hit just because a file went
    // away.
    for path in input_paths(spec) {
        update_mtime(&mut hasher, &path);
    }

    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

/// Read a previously-written hash from disk. Returns `None` if the file
/// is missing or unreadable; callers treat any `None` as a cache miss.
pub fn read_cached_hash(cache_file: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(cache_file).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Persist a hash next to the rendered MP4. Errors are non-fatal — a
/// failed write means future runs miss the cache, not that the render
/// itself fails.
pub fn write_hash(cache_file: &Path, hash: &str) -> std::io::Result<()> {
    if let Some(parent) = cache_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(cache_file, hash.as_bytes())
}

fn input_paths(spec: &SceneSpec) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(4 + spec.scenes.len());
    paths.push(spec.audio.wav.clone());
    if let Some(p) = &spec.audio.peaks {
        paths.push(p.clone());
    }
    if let Background::Image { src, .. } = &spec.background {
        paths.push(src.clone());
    }
    for scene in &spec.scenes {
        if let Scene::Paragraph { tile: Some(p), .. } = scene {
            paths.push(p.clone());
        }
    }
    if let Some(c) = &spec.captions {
        paths.push(c.src.clone());
    }
    paths
}

fn update_mtime(hasher: &mut Sha256, path: &Path) {
    let nanos = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // The path string keeps the hash sensitive to renames (same
    // mtime, different file).
    let path_str = path.to_string_lossy();
    hasher.update(path_str.as_bytes());
    hasher.update(b"\0");
    hasher.update(nanos.to_le_bytes());
    hasher.update(b"\0");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::spec::{
        AudioRef, Background, ChapterMeta, Output, Scene, SceneSpec,
    };

    fn fixture_spec(out: &Path) -> SceneSpec {
        SceneSpec::new(
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
            Output::hd_1080(out.to_path_buf(), 24),
        )
        .push(Scene::Title {
            start_ms: 0,
            end_ms: 4_000,
            title: "Chapter 1".into(),
            subtitle: None,
        })
    }

    #[test]
    fn hash_is_stable_across_calls() {
        let spec = fixture_spec(Path::new("/tmp/a.mp4"));
        let h1 = compute_spec_hash(&spec, REVIDEO_PATH_LABEL);
        let h2 = compute_spec_hash(&spec, REVIDEO_PATH_LABEL);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn hash_ignores_output_path() {
        // Two specs that differ only in where they write should hit
        // the same hash — the *content* is identical.
        let a = fixture_spec(Path::new("/tmp/a.mp4"));
        let b = fixture_spec(Path::new("/tmp/different/path/b.mp4"));
        assert_eq!(
            compute_spec_hash(&a, REVIDEO_PATH_LABEL),
            compute_spec_hash(&b, REVIDEO_PATH_LABEL)
        );
    }

    #[test]
    fn hash_changes_with_fps() {
        let mut a = fixture_spec(Path::new("/tmp/a.mp4"));
        let mut b = fixture_spec(Path::new("/tmp/a.mp4"));
        a.output.fps = 24;
        b.output.fps = 30;
        assert_ne!(
            compute_spec_hash(&a, REVIDEO_PATH_LABEL),
            compute_spec_hash(&b, REVIDEO_PATH_LABEL)
        );
    }

    #[test]
    fn hash_changes_with_chapter_title() {
        let mut a = fixture_spec(Path::new("/tmp/a.mp4"));
        let mut b = fixture_spec(Path::new("/tmp/a.mp4"));
        a.chapter.title = "One".into();
        b.chapter.title = "Two".into();
        assert_ne!(
            compute_spec_hash(&a, REVIDEO_PATH_LABEL),
            compute_spec_hash(&b, REVIDEO_PATH_LABEL)
        );
    }

    #[test]
    fn hash_changes_with_render_path() {
        // Same spec, different render path label → different hash.
        // Flipping LISTENAI_ANIMATE_FAST_PATH must invalidate the
        // cache so the next render produces an MP4 with the new
        // path's pixel output.
        let spec = fixture_spec(Path::new("/tmp/a.mp4"));
        assert_ne!(
            compute_spec_hash(&spec, REVIDEO_PATH_LABEL),
            compute_spec_hash(&spec, FFMPEG_PATH_LABEL)
        );
    }

    #[test]
    fn cache_path_appends_hash_extension() {
        let p = cache_path(Path::new("/tmp/dir/ch-3.video.mp4"));
        assert_eq!(p, PathBuf::from("/tmp/dir/ch-3.video.mp4.hash"));
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cache_file = dir.path().join("ch-1.video.mp4.hash");
        write_hash(&cache_file, "abc123").unwrap();
        assert_eq!(read_cached_hash(&cache_file).as_deref(), Some("abc123"));
    }

    #[test]
    fn read_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let cache_file = dir.path().join("nope.hash");
        assert_eq!(read_cached_hash(&cache_file), None);
    }
}
