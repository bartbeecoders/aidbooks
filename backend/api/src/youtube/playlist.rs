//! YouTube Data API v3 playlist helpers.
//!
//! Two operations only:
//!   * [`create_playlist`] — POST `/youtube/v3/playlists` to mint a new
//!     playlist on the authenticated user's channel.
//!   * [`add_video`] — POST `/youtube/v3/playlistItems` to append a video.
//!
//! Both endpoints want the broader `youtube` scope; `youtube.upload` alone
//! is not enough. The OAuth scope list at module top level is responsible
//! for asking for it, and re-consenting accounts that connected before this
//! module landed.

use std::time::Duration;

use listenai_core::{Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const PLAYLISTS_URL: &str =
    "https://www.googleapis.com/youtube/v3/playlists?part=snippet,status";
const PLAYLIST_ITEMS_URL: &str =
    "https://www.googleapis.com/youtube/v3/playlistItems?part=snippet";
const PLAYLISTS_DELETE_URL: &str = "https://www.googleapis.com/youtube/v3/playlists";

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: String,
    pub url: String,
}

/// Create a YouTube playlist. Pass `podcast = true` to also mark the
/// playlist as a podcast (`status.podcastStatus = "enabled"`), so it
/// surfaces in the YouTube Music Podcasts tab + carries podcast-shaped
/// metadata. Regular (non-podcast) callers pass `false` and leave
/// `podcastStatus` unset.
pub async fn create_playlist(
    access_token: &str,
    title: &str,
    description: &str,
    privacy_status: &str,
    default_language: Option<&str>,
    podcast: bool,
) -> Result<Playlist> {
    #[derive(Serialize)]
    struct Snippet<'a> {
        title: &'a str,
        description: &'a str,
        #[serde(rename = "defaultLanguage", skip_serializing_if = "Option::is_none")]
        default_language: Option<&'a str>,
    }
    #[derive(Serialize)]
    struct Status<'a> {
        #[serde(rename = "privacyStatus")]
        privacy_status: &'a str,
        /// `"enabled"` to designate the playlist as a podcast. Omitted
        /// for plain playlists — YouTube treats absence as "disabled"
        /// without flipping the value on existing rows.
        #[serde(rename = "podcastStatus", skip_serializing_if = "Option::is_none")]
        podcast_status: Option<&'a str>,
    }
    #[derive(Serialize)]
    struct Body<'a> {
        snippet: Snippet<'a>,
        status: Status<'a>,
    }
    #[derive(Deserialize)]
    struct Resource {
        id: String,
    }

    let body = Body {
        snippet: Snippet {
            title,
            description,
            default_language,
        },
        status: Status {
            privacy_status,
            podcast_status: if podcast { Some("enabled") } else { None },
        },
    };
    let resp = http()?
        .post(PLAYLISTS_URL)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt playlist create: {e}")))?;

    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("yt playlist create read: {e}")))?;
    if status.as_u16() == 401 {
        return Err(Error::Unauthorized);
    }
    if !status.is_success() {
        let preview = String::from_utf8_lossy(&bytes);
        // Same shape as update — empty + podcast → failedPrecondition.
        if status.as_u16() == 400
            && (preview.contains("failedPrecondition")
                || preview.contains("FAILED_PRECONDITION"))
        {
            return Err(Error::Conflict(format!(
                "yt playlist create precondition failed: {}",
                preview.chars().take(400).collect::<String>()
            )));
        }
        return Err(Error::Upstream(format!(
            "yt playlist create {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    let resource: Resource = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Upstream(format!("yt playlist create json: {e}")))?;
    let url = format!("https://www.youtube.com/playlist?list={}", resource.id);
    Ok(Playlist {
        id: resource.id,
        url,
    })
}

/// Update an existing playlist's title + description in place. The
/// `privacy_status` is also re-asserted because the YouTube API rejects
/// partial `PUT` payloads — every required field on `snippet`/`status`
/// has to come along. Pass `podcast = true` to keep the row marked as a
/// podcast on every update; this matches `create_playlist`'s semantics
/// so the field doesn't drift back to disabled across edits.
///
/// Errors:
///   * `Error::NotFound` — playlist deleted on YouTube; caller should
///     clear the reference and mint a new one.
///   * `Error::Conflict` — `failedPrecondition`. Most commonly: trying
///     to enable `podcastStatus` on a playlist with no videos. Caller
///     can retry with `podcast = false` to apply the metadata at least.
pub async fn update_playlist(
    access_token: &str,
    playlist_id: &str,
    title: &str,
    description: &str,
    privacy_status: &str,
    default_language: Option<&str>,
    podcast: bool,
) -> Result<()> {
    #[derive(Serialize)]
    struct Snippet<'a> {
        title: &'a str,
        description: &'a str,
        #[serde(rename = "defaultLanguage", skip_serializing_if = "Option::is_none")]
        default_language: Option<&'a str>,
    }
    #[derive(Serialize)]
    struct Status<'a> {
        #[serde(rename = "privacyStatus")]
        privacy_status: &'a str,
        #[serde(rename = "podcastStatus", skip_serializing_if = "Option::is_none")]
        podcast_status: Option<&'a str>,
    }
    #[derive(Serialize)]
    struct Body<'a> {
        id: &'a str,
        snippet: Snippet<'a>,
        status: Status<'a>,
    }

    let body = Body {
        id: playlist_id,
        snippet: Snippet {
            title,
            description,
            default_language,
        },
        status: Status {
            privacy_status,
            podcast_status: if podcast { Some("enabled") } else { None },
        },
    };
    let resp = http()?
        .put(PLAYLISTS_URL)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt playlist update: {e}")))?;

    let status = resp.status();
    if status.as_u16() == 401 {
        return Err(Error::Unauthorized);
    }
    if status.as_u16() == 404 {
        // Playlist gone — likely deleted at YouTube. Caller decides what
        // to do with it (typically: clear our reference and re-mint).
        return Err(Error::NotFound {
            resource: format!("playlist:{playlist_id}"),
        });
    }
    if !status.is_success() {
        let bytes = resp.bytes().await.unwrap_or_default();
        let preview = String::from_utf8_lossy(&bytes);
        // YouTube returns `failedPrecondition` (status FAILED_PRECONDITION)
        // when designating an empty playlist as a podcast — the playlist
        // must contain at least one episode first. Surface as Conflict so
        // the caller can handle it without parsing strings.
        if status.as_u16() == 400
            && (preview.contains("failedPrecondition")
                || preview.contains("FAILED_PRECONDITION"))
        {
            return Err(Error::Conflict(format!(
                "yt playlist update precondition failed: {}",
                preview.chars().take(400).collect::<String>()
            )));
        }
        return Err(Error::Upstream(format!(
            "yt playlist update {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    Ok(())
}

/// Delete a playlist. Returns `Ok(())` for 200/204 and 404 (already gone),
/// `Err(Unauthorized)` for 401, and `Err(Upstream)` otherwise.
pub async fn delete_playlist(access_token: &str, playlist_id: &str) -> Result<()> {
    let resp = http()?
        .delete(PLAYLISTS_DELETE_URL)
        .query(&[("id", playlist_id)])
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt playlist delete: {e}")))?;

    let status = resp.status();
    if status.as_u16() == 401 {
        return Err(Error::Unauthorized);
    }
    // 404 = playlist already gone; treat as success.
    if status.is_success() || status.as_u16() == 404 {
        return Ok(());
    }
    let bytes = resp.bytes().await.unwrap_or_default();
    let preview = String::from_utf8_lossy(&bytes);
    Err(Error::Upstream(format!(
        "yt playlist delete {status}: {}",
        preview.chars().take(400).collect::<String>()
    )))
}

/// Append a video to a playlist. `position` is 0-based; pass `None` to let
/// YouTube append at the end.
pub async fn add_video(
    access_token: &str,
    playlist_id: &str,
    video_id: &str,
    position: Option<u32>,
) -> Result<()> {
    #[derive(Serialize)]
    struct ResourceId<'a> {
        kind: &'a str,
        #[serde(rename = "videoId")]
        video_id: &'a str,
    }
    #[derive(Serialize)]
    struct Snippet<'a> {
        #[serde(rename = "playlistId")]
        playlist_id: &'a str,
        #[serde(rename = "resourceId")]
        resource_id: ResourceId<'a>,
        #[serde(skip_serializing_if = "Option::is_none")]
        position: Option<u32>,
    }
    #[derive(Serialize)]
    struct Body<'a> {
        snippet: Snippet<'a>,
    }

    let body = Body {
        snippet: Snippet {
            playlist_id,
            resource_id: ResourceId {
                kind: "youtube#video",
                video_id,
            },
            position,
        },
    };
    let resp = http()?
        .post(PLAYLIST_ITEMS_URL)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt playlistItems: {e}")))?;

    let status = resp.status();
    if status.as_u16() == 401 {
        return Err(Error::Unauthorized);
    }
    if !status.is_success() {
        let bytes = resp.bytes().await.unwrap_or_default();
        let preview = String::from_utf8_lossy(&bytes);
        return Err(Error::Upstream(format!(
            "yt playlistItems {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    Ok(())
}

fn http() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("listenai-api/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Other(anyhow::anyhow!("yt http client: {e}")))
}
