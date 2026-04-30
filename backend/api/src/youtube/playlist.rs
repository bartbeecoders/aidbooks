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

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: String,
    pub url: String,
}

pub async fn create_playlist(
    access_token: &str,
    title: &str,
    description: &str,
    privacy_status: &str,
    default_language: Option<&str>,
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
        status: Status { privacy_status },
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
