//! YouTube Data API v3 resumable upload.
//!
//! Two-step protocol:
//!   1. POST `/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status`
//!      with the JSON metadata. Response carries a `Location:` header that is
//!      the per-upload URL.
//!   2. PUT the MP4 bytes to that URL in chunks. Each chunk uses
//!      `Content-Range: bytes <start>-<end>/<total>`. A 308 reply means
//!      "more please"; the `Range:` header tells us what was last accepted.
//!
//! Chunked uploads keep memory usage bounded for large books and let us
//! emit progress events as bytes go out the door.

use std::path::Path;
use std::time::{Duration, Instant};

use listenai_core::{Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tracing::info;

const VIDEOS_URL: &str =
    "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status";

/// 8 MiB — the YouTube docs recommend a multiple of 256 KiB. Big enough to
/// keep request count low (a 60-min audiobook is ~50 MB → 6-7 chunks); small
/// enough that progress feels live.
const CHUNK_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct VideoMetadata {
    pub snippet: Snippet,
    pub status: VideoStatus,
}

#[derive(Debug, Serialize)]
pub struct Snippet {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    #[serde(rename = "categoryId")]
    pub category_id: String,
    #[serde(rename = "defaultLanguage", skip_serializing_if = "Option::is_none")]
    pub default_language: Option<String>,
    #[serde(rename = "defaultAudioLanguage", skip_serializing_if = "Option::is_none")]
    pub default_audio_language: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VideoStatus {
    #[serde(rename = "privacyStatus")]
    pub privacy_status: String,
    #[serde(rename = "selfDeclaredMadeForKids")]
    pub self_declared_made_for_kids: bool,
}

#[derive(Debug, Deserialize)]
struct VideoResource {
    id: String,
}

#[derive(Debug, Clone)]
pub struct UploadResult {
    pub video_id: String,
}

/// Open an upload session: returns the per-upload URL Google wants the bytes
/// PUT to.
pub async fn start_session(
    access_token: &str,
    metadata: &VideoMetadata,
    total_bytes: u64,
) -> Result<String> {
    let http = build_client()?;
    let body = serde_json::to_vec(metadata)
        .map_err(|e| Error::Other(anyhow::anyhow!("yt metadata json: {e}")))?;
    let resp = http
        .post(VIDEOS_URL)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json; charset=UTF-8")
        .header("X-Upload-Content-Type", "video/mp4")
        .header("X-Upload-Content-Length", total_bytes.to_string())
        .body(body)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt start session: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let bytes = resp.bytes().await.unwrap_or_default();
        let preview = String::from_utf8_lossy(&bytes);
        return Err(Error::Upstream(format!(
            "yt start session {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| Error::Upstream("yt start session: no Location header".into()))?;
    Ok(location)
}

/// Stream the MP4 file at `path` to `upload_url` in `CHUNK_SIZE` chunks.
/// `progress` is called both at the *start* of each chunk (so the UI moves
/// forward when a slow chunk begins, not just when it finishes) and at the
/// end. Errors from the callback are ignored — the upload is the load-
/// bearing concern.
pub async fn upload_file<F, Fut>(
    upload_url: &str,
    path: &Path,
    mut progress: F,
) -> Result<UploadResult>
where
    F: FnMut(u64, u64) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("open mp4 {path:?}: {e}")))?;
    let total = file
        .metadata()
        .await
        .map(|m| m.len())
        .map_err(|e| Error::Other(anyhow::anyhow!("stat mp4: {e}")))?;
    if total == 0 {
        return Err(Error::Other(anyhow::anyhow!("mp4 file is empty")));
    }

    info!(
        bytes = total,
        chunk = CHUNK_SIZE,
        chunks = total.div_ceil(CHUNK_SIZE as u64),
        "yt upload: starting"
    );
    let started = Instant::now();

    let http = build_client()?;
    let mut offset: u64 = 0;
    let mut buf = vec![0u8; CHUNK_SIZE];
    // Initial 0% emission so the UI knows the upload has started even
    // before the first chunk completes.
    progress(0, total).await;

    loop {
        if offset >= total {
            // Shouldn't happen — final chunk normally returns the resource.
            return Err(Error::Upstream(
                "yt upload exhausted without final response".into(),
            ));
        }
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| Error::Other(anyhow::anyhow!("seek mp4: {e}")))?;
        let chunk_len = std::cmp::min(CHUNK_SIZE as u64, total - offset) as usize;
        // read_exact rather than read so a short read on a partial buffer
        // doesn't silently truncate the chunk.
        file.read_exact(&mut buf[..chunk_len])
            .await
            .map_err(|e| Error::Other(anyhow::anyhow!("read mp4 chunk: {e}")))?;

        let end = offset + chunk_len as u64 - 1;
        let range_header = format!("bytes {offset}-{end}/{total}");

        // Emit "starting chunk" progress before the network call so a
        // slow PUT doesn't look like a hang on the UI side.
        progress(offset, total).await;
        let chunk_started = Instant::now();
        let resp = http
            .put(upload_url)
            .header("Content-Length", chunk_len.to_string())
            .header("Content-Range", &range_header)
            .body(buf[..chunk_len].to_vec())
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("yt upload chunk: {e}")))?;

        let status = resp.status();
        let code = status.as_u16();

        // 200/201 → upload complete; body is the Video resource.
        if status.is_success() {
            let body = resp
                .bytes()
                .await
                .map_err(|e| Error::Upstream(format!("yt upload response read: {e}")))?;
            let resource: VideoResource = serde_json::from_slice(&body)
                .map_err(|e| Error::Upstream(format!("yt upload response json: {e}")))?;
            progress(total, total).await;
            info!(
                bytes = total,
                elapsed_ms = started.elapsed().as_millis() as u64,
                video_id = %resource.id,
                "yt upload: complete"
            );
            return Ok(UploadResult {
                video_id: resource.id,
            });
        }

        // 308 → "Resume Incomplete". Google may not have committed the full
        // chunk we just sent (rare for chunked PUTs at this size, but happens
        // on flaky links). Use the Range header as the new offset.
        if code == 308 {
            let next_offset = match resp
                .headers()
                .get("range")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_range_end)
            {
                Some(end_byte) => end_byte + 1,
                None => offset + chunk_len as u64,
            };
            // Defensive: ensure we're advancing. A 308 with Range pointing at
            // an earlier byte would loop forever.
            if next_offset <= offset {
                return Err(Error::Upstream(format!(
                    "yt upload 308 with non-advancing range (offset={offset}, next={next_offset})"
                )));
            }
            info!(
                from = offset,
                to = next_offset,
                of = total,
                ms = chunk_started.elapsed().as_millis() as u64,
                "yt upload: chunk accepted"
            );
            offset = next_offset;
            progress(offset, total).await;
            continue;
        }

        let bytes = resp.bytes().await.unwrap_or_default();
        let preview = String::from_utf8_lossy(&bytes);
        return Err(Error::Upstream(format!(
            "yt upload chunk {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
}

/// Attach an SRT caption track to an existing video via the
/// `captions.insert` endpoint. Single multipart/related request — the
/// caption files are tiny enough that there's no point in resumable
/// uploads.
///
/// Returns `Ok(())` on success. Callers treat failure as best-effort:
/// the video is already published, missing captions shouldn't roll back
/// the publish.
pub async fn upload_caption(
    access_token: &str,
    video_id: &str,
    language: &str,
    name: &str,
    srt_body: &str,
) -> Result<()> {
    if srt_body.trim().is_empty() {
        return Ok(());
    }

    // Multipart/related framing per Google's "multipart upload" docs. The
    // boundary is opaque; pick something unlikely to collide with SRT
    // content (which is mostly digits + ASCII letters).
    const BOUNDARY: &str = "aidbooks-caption-boundary-9F8E7D6C5B4A";
    let url = "https://www.googleapis.com/upload/youtube/v3/captions\
         ?part=snippet&uploadType=multipart";

    let snippet = serde_json::json!({
        "snippet": {
            "videoId": video_id,
            "language": language,
            "name": name,
        }
    });
    let snippet_bytes = serde_json::to_vec(&snippet)
        .map_err(|e| Error::Other(anyhow::anyhow!("yt caption json: {e}")))?;

    let mut body: Vec<u8> = Vec::with_capacity(snippet_bytes.len() + srt_body.len() + 256);
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(&snippet_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    // YouTube accepts `application/octet-stream` and sniffs the format
    // from the body. Could also send `text/srt` but octet-stream avoids
    // any debate about charset/MIME variants.
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(srt_body.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    let http = build_client()?;
    let resp = http
        .post(url)
        .bearer_auth(access_token)
        .header(
            "Content-Type",
            format!("multipart/related; boundary={BOUNDARY}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt caption upload: {e}")))?;

    let status = resp.status();
    if status.as_u16() == 401 {
        return Err(Error::Unauthorized);
    }
    if !status.is_success() {
        let bytes = resp.bytes().await.unwrap_or_default();
        let preview = String::from_utf8_lossy(&bytes);
        return Err(Error::Upstream(format!(
            "yt caption upload {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    Ok(())
}

fn parse_range_end(range: &str) -> Option<u64> {
    // YouTube returns `bytes=0-<end>`; tolerate either separator.
    let after_eq = range.split_once('=').map(|(_, v)| v).unwrap_or(range);
    let end = after_eq.split_once('-').map(|(_, v)| v).unwrap_or(after_eq);
    end.trim().parse().ok()
}

fn build_client() -> Result<Client> {
    Client::builder()
        // Per-request cap. An 8 MiB chunk needs ~22 kbps upstream to clear
        // 5 minutes; the 10-minute ceiling tolerates ~half that and still
        // bounds genuinely runaway connections so the worker doesn't sit
        // forever on a black hole.
        .timeout(Duration::from_secs(10 * 60))
        // Detect drops sooner than the default keepalive so a half-closed
        // socket doesn't burn the full timeout above.
        .tcp_keepalive(Duration::from_secs(30))
        .user_agent(concat!("listenai-api/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Other(anyhow::anyhow!("yt http client: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_range_end_handles_canonical_form() {
        assert_eq!(parse_range_end("bytes=0-262143"), Some(262143));
        assert_eq!(parse_range_end("0-100"), Some(100));
        assert_eq!(parse_range_end(""), None);
    }
}
