//! OAuth 2.0 + Channel API helpers for the YouTube integration.
//!
//! Operations:
//!   * [`build_consent_url`] — produce the URL we redirect the user to.
//!   * [`exchange_code`] — first-leg `code → access + refresh` swap.
//!   * [`refresh_access`] — `refresh → fresh access token`.
//!   * [`fetch_channel`] — call `/youtube/v3/channels?mine=true` so we can
//!     store the channel id/title at connect time.
//!   * [`revoke`] — explicit revoke on disconnect.
//!
//! Endpoint URLs are hard-coded to the canonical Google ones; making them
//! configurable would only complicate the happy path.

use std::time::Duration;

use listenai_core::{Error, Result};
use reqwest::Client;
use serde::Deserialize;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const CHANNELS_URL: &str = "https://www.googleapis.com/youtube/v3/channels";

// Three scopes cover the full publish surface:
//   * `youtube.upload`   — video uploads (resumable upload session).
//   * `youtube`          — playlist writes (`/playlists`, `/playlistItems`).
//   * `youtube.force-ssl`— captions writes (`/captions`); Google requires
//     this exact scope for the captions endpoint.
// `youtube.upload` stays listed explicitly so the consent screen names the
// upload action plainly.
pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/youtube.upload",
    "https://www.googleapis.com/auth/youtube",
    "https://www.googleapis.com/auth/youtube.force-ssl",
];

/// Result of swapping a `code` for tokens. `refresh_token` is `None` when
/// Google decides not to issue one (e.g. user already granted offline access
/// to this client and we forgot `prompt=consent`). Treated as a hard error
/// upstream because we can't function without it.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    /// Lifetime of `access_token` in seconds. Currently unused — every
    /// publish job calls [`refresh_access`] up front rather than caching.
    #[allow(dead_code)]
    pub expires_in: i64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Space-separated list of scopes Google actually granted. Currently
    /// unused — we already know which scopes we asked for.
    #[allow(dead_code)]
    #[serde(default)]
    pub scope: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub token_type: Option<String>,
}

/// Subset of `/youtube/v3/channels?mine=true` we care about.
#[derive(Debug, Deserialize)]
struct ChannelListResponse {
    items: Vec<ChannelItem>,
}
#[derive(Debug, Deserialize)]
struct ChannelItem {
    id: String,
    snippet: ChannelSnippet,
}
#[derive(Debug, Deserialize)]
struct ChannelSnippet {
    title: String,
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: String,
    pub title: String,
}

/// Build the `accounts.google.com/o/oauth2/v2/auth` URL the user is sent to.
/// `state` is opaque; we use it for CSRF + user binding (lookup table).
pub fn build_consent_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scopes: &[&str],
) -> String {
    let scope = scopes.join(" ");
    let mut qs = form_urlencoded::Serializer::new(String::new());
    qs.append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &scope)
        .append_pair("access_type", "offline")
        // Force the consent screen so Google always issues a refresh token,
        // even if the user previously authorised this client.
        .append_pair("prompt", "consent")
        .append_pair("include_granted_scopes", "true")
        .append_pair("state", state);
    format!("{AUTH_URL}?{}", qs.finish())
}

/// Build a small reqwest client with sane defaults. Reused across calls so
/// the pool stays warm, but cheap enough to construct per-call too.
fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("listenai-api/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Other(anyhow::anyhow!("yt http client: {e}")))
}

pub async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
) -> Result<TokenResponse> {
    let http = http_client()?;
    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
            ("code", code),
        ])
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt token exchange: {e}")))?;

    decode_token(resp).await
}

/// Trade a stored refresh token for a fresh access token. Note Google does
/// NOT return a new refresh token here unless rotation was enabled — leave
/// the stored one intact.
pub async fn refresh_access(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenResponse> {
    let http = http_client()?;
    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt token refresh: {e}")))?;

    decode_token(resp).await
}

pub async fn fetch_channel(access_token: &str) -> Result<Channel> {
    let http = http_client()?;
    let resp = http
        .get(CHANNELS_URL)
        .query(&[("part", "snippet"), ("mine", "true")])
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt channels: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("yt channels read: {e}")))?;
    if !status.is_success() {
        let preview = String::from_utf8_lossy(&bytes);
        return Err(Error::Upstream(format!(
            "yt channels {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    let body: ChannelListResponse = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Upstream(format!("yt channels json: {e}")))?;
    let item = body
        .items
        .into_iter()
        .next()
        .ok_or_else(|| Error::Upstream("yt channels: no channel for this account".into()))?;
    Ok(Channel {
        id: item.id,
        title: item.snippet.title,
    })
}

/// Best-effort revoke. Google returns 200 on success and 400 when the token
/// is already invalid; we treat both as success since the user-facing intent
/// (disconnect) is satisfied either way.
pub async fn revoke(token: &str) -> Result<()> {
    let http = http_client()?;
    let resp = http
        .post(REVOKE_URL)
        .form(&[("token", token)])
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt revoke: {e}")))?;
    if !resp.status().is_success() {
        // 400 invalid_token is fine — already revoked.
        let s = resp.status();
        if s.as_u16() != 400 {
            return Err(Error::Upstream(format!("yt revoke status {s}")));
        }
    }
    Ok(())
}

async fn decode_token(resp: reqwest::Response) -> Result<TokenResponse> {
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("yt token read: {e}")))?;
    if !status.is_success() {
        let preview = String::from_utf8_lossy(&bytes);
        // invalid_grant → user revoked from Google; bubble as Unauthorized so
        // the caller knows to delete the stored row + ask for reconnect.
        if preview.contains("invalid_grant") {
            return Err(Error::Unauthorized);
        }
        return Err(Error::Upstream(format!(
            "yt token {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    serde_json::from_slice::<TokenResponse>(&bytes)
        .map_err(|e| Error::Upstream(format!("yt token json: {e}")))
}
