//! Tinyfish CLI wrapper for the songbook category.
//!
//! Songbook outlines need lyrics + artist context + song meaning so the
//! LLM can plan an audiobook *about* a song. We get them by shelling
//! out to the `tinyfish` CLI:
//!
//! 1. `tinyfish search query "<q> lyrics"` → ranked URLs.
//! 2. `tinyfish fetch content get --format markdown <urls>` → cleaned
//!    page text we hand to the prompt.
//!
//! This is best-effort: an empty `TINYFISH_API_KEY`, a missing CLI, a
//! network blip, or unparseable JSON all collapse to an empty
//! `SongInfo` with a `warn!` log. Songbooks must still produce *some*
//! outline rather than 502 the create flow.

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;
use tracing::warn;

use crate::state::AppState;

/// What the LLM gets passed into the songbook outline prompt. Empty
/// strings are valid — the prompt is written to degrade gracefully.
#[derive(Debug, Default, Clone)]
pub struct SongInfo {
    pub lyrics: String,
    pub artist_bio: String,
    pub song_meaning: String,
}

/// Per-CLI-call wall clock. The fetch step pulls multiple lyrics
/// pages and waits on their JS, so 20 s wasn't enough on cold cache.
/// 60 s lines up with the upstream LLM timeouts and keeps the
/// synchronous create handler responsive.
const TINYFISH_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Best-effort fetch of lyrics + artist bio + song-meaning text for
/// `query` (e.g. `"Bohemian Rhapsody — Queen"`). On *any* failure we
/// log a warn and return `SongInfo::default()`.
pub async fn fetch_song_info(state: &AppState, query: &str) -> SongInfo {
    let cfg = state.config();
    let key = cfg.tinyfish_api_key.trim();
    if key.is_empty() {
        warn!("tinyfish: no key, songbook running without lyrics");
        return SongInfo::default();
    }
    let bin = cfg.tinyfish_bin.trim();
    if bin.is_empty() {
        warn!("tinyfish: tinyfish_bin empty, songbook running without lyrics");
        return SongInfo::default();
    }

    let urls = match search_urls(bin, key, query).await {
        Ok(u) if !u.is_empty() => u,
        Ok(_) => {
            warn!(query, "tinyfish search returned no results");
            return SongInfo::default();
        }
        Err(e) => {
            warn!(error = %e, query, "tinyfish search failed");
            return SongInfo::default();
        }
    };

    match fetch_pages(bin, key, &urls).await {
        Ok(info) => info,
        Err(e) => {
            warn!(error = %e, "tinyfish fetch failed");
            SongInfo::default()
        }
    }
}

/// Run `tinyfish search query "<q> lyrics"` and pull up to 3 result
/// URLs. We bias toward known lyrics aggregators when present so the
/// fetch step lands on pages with the actual lyrics body.
async fn search_urls(bin: &str, key: &str, query: &str) -> Result<Vec<String>, String> {
    let q = format!("{} lyrics", query.trim());
    let output = run(bin, key, &["search", "query", &q]).await?;

    #[derive(Deserialize)]
    struct SearchEnvelope {
        #[serde(default)]
        results: Vec<SearchResult>,
    }
    #[derive(Deserialize)]
    struct SearchResult {
        url: Option<String>,
    }

    let env: SearchEnvelope =
        serde_json::from_slice(&output).map_err(|e| format!("parse search json: {e}"))?;
    let mut all: Vec<String> = env
        .results
        .into_iter()
        .filter_map(|r| r.url.filter(|s| !s.trim().is_empty()))
        .collect();

    // Stable rerank: known lyrics sources first, others keep order.
    const PREFERRED: &[&str] = &[
        "genius.com",
        "azlyrics.com",
        "songmeanings.com",
        "lyrics.com",
        "songfacts.com",
    ];
    all.sort_by_key(|u| {
        PREFERRED
            .iter()
            .position(|p| u.contains(p))
            .unwrap_or(usize::MAX)
    });
    all.truncate(3);
    Ok(all)
}

/// Fetch the cleaned markdown body of each url and route it into the
/// `SongInfo` slot that best matches the source.
async fn fetch_pages(bin: &str, key: &str, urls: &[String]) -> Result<SongInfo, String> {
    let mut args: Vec<&str> = vec!["fetch", "content", "get", "--format", "markdown"];
    for u in urls {
        args.push(u);
    }
    let output = run(bin, key, &args).await?;

    // The fetch endpoint accepts multiple URLs and returns one result
    // per URL. Tolerate both an array shape and an object-with-results
    // shape — the CLI evolves and both have been seen.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Envelope {
        List(Vec<Page>),
        Wrapped { results: Vec<Page> },
    }
    #[derive(Deserialize)]
    struct Page {
        #[serde(default)]
        url: String,
        #[serde(default)]
        text: String,
    }

    let env: Envelope =
        serde_json::from_slice(&output).map_err(|e| format!("parse fetch json: {e}"))?;
    let pages: Vec<Page> = match env {
        Envelope::List(v) => v,
        Envelope::Wrapped { results } => results,
    };

    // Bucket each fetched page by host so we can line it up with the
    // right SongInfo field. Lyrics aggregators → lyrics; meaning sites
    // → song_meaning; everything else (encyclopedic / news) →
    // artist_bio. Concatenate within each bucket so multiple hits don't
    // overwrite each other.
    let mut lyrics_buf = String::new();
    let mut meaning_buf = String::new();
    let mut bio_buf = String::new();
    for p in pages {
        let body = trim_to(&p.text, 8_000);
        if body.is_empty() {
            continue;
        }
        let host = p.url.to_lowercase();
        let bucket = if host.contains("genius.com")
            || host.contains("azlyrics.com")
            || host.contains("lyrics.com")
        {
            &mut lyrics_buf
        } else if host.contains("songmeanings.com") || host.contains("songfacts.com") {
            &mut meaning_buf
        } else {
            &mut bio_buf
        };
        if !bucket.is_empty() {
            bucket.push_str("\n\n---\n\n");
        }
        bucket.push_str(&body);
    }

    Ok(SongInfo {
        lyrics: lyrics_buf,
        artist_bio: bio_buf,
        song_meaning: meaning_buf,
    })
}

/// Best-effort: search Tinyfish for a YouTube `watch` URL of the
/// (presumed official) audio for `query`. Returns `None` (with a
/// `warn!`) on any error so the snippet job can fall back to "no
/// audio available" gracefully.
pub async fn find_youtube_url(state: &AppState, query: &str) -> Option<String> {
    let cfg = state.config();
    let key = cfg.tinyfish_api_key.trim();
    if key.is_empty() {
        warn!("tinyfish: no key, snippet job skipping youtube lookup");
        return None;
    }
    let bin = cfg.tinyfish_bin.trim();
    if bin.is_empty() {
        warn!("tinyfish: tinyfish_bin empty, snippet job skipping youtube lookup");
        return None;
    }

    let q = format!("{} official audio youtube", query.trim());
    let bytes = match run(bin, key, &["search", "query", &q]).await {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "tinyfish youtube search failed");
            return None;
        }
    };

    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        results: Vec<Item>,
    }
    #[derive(Deserialize)]
    struct Item {
        url: Option<String>,
    }

    let env: Envelope = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "tinyfish youtube search: parse failed");
            return None;
        }
    };
    env.results.into_iter().filter_map(|r| r.url).find(|u| {
        let lo = u.to_lowercase();
        (lo.contains("youtube.com/watch") || lo.contains("youtu.be/")) && !lo.contains("/shorts/")
    })
}

/// Spawn `bin <args>` with `TINYFISH_API_KEY` set, capped by
/// `TINYFISH_CALL_TIMEOUT`. Returns stdout bytes on success.
async fn run(bin: &str, key: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env("TINYFISH_API_KEY", key)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let fut = cmd.output();
    let out = tokio::time::timeout(TINYFISH_CALL_TIMEOUT, fut)
        .await
        .map_err(|_| format!("timeout after {:?}", TINYFISH_CALL_TIMEOUT))?
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

fn trim_to(s: &str, max: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    trimmed.chars().take(max).collect()
}
