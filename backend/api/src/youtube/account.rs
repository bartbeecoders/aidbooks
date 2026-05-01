//! Shared YouTube-account helpers used by both HTTP handlers and the
//! background publish job.
//!
//! Resolving an access token requires three steps:
//!   1. Look up the `youtube_account` row for the user.
//!   2. Decrypt the stored refresh token with `Config.password_pepper`.
//!   3. Trade the refresh token for a fresh access token (Google's
//!      access tokens are short-lived, so we don't cache them).
//!
//! The publish job has its own copy of this for legacy reasons; new
//! callers (e.g. the podcast → playlist sync) should use [`access_token`]
//! here so behaviour stays consistent.

use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;

use crate::state::AppState;
use crate::youtube::{encrypt, oauth};

/// Decrypt + refresh the user's YouTube refresh token. Returns:
///   * `Ok(Some(token))` — connected and the token is good to use.
///   * `Ok(None)`        — the user has not connected a YouTube channel.
///   * `Err(Unauthorized)` — the user revoked at Google; caller should
///     clean up the local row + ask for reconnect.
///   * `Err(_)`          — transient upstream / database error.
pub async fn access_token(state: &AppState, user: &UserId) -> Result<Option<String>> {
    #[derive(Debug, Deserialize)]
    struct Row {
        refresh_token_enc: String,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT refresh_token_enc FROM youtube_account WHERE owner = user:`{}` LIMIT 1",
            user.0
        ))
        .await
        .map_err(|e| Error::Database(format!("yt token load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt token load (decode): {e}")))?;
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let pepper = state.config().password_pepper.as_bytes();
    let refresh = encrypt::decrypt(&row.refresh_token_enc, pepper)?;
    let cfg = state.config();
    let resp =
        oauth::refresh_access(&cfg.youtube_client_id, &cfg.youtube_client_secret, &refresh).await?;
    Ok(Some(resp.access_token))
}

/// Best-effort cleanup of the local `youtube_account` row. Used when an
/// upstream call returns 401 / `invalid_grant` so the next API request
/// from the user gets a "please reconnect" surface.
pub async fn drop_account(state: &AppState, user: &UserId) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "DELETE youtube_account WHERE owner = user:`{}`",
            user.0
        ))
        .await
        .map_err(|e| Error::Database(format!("yt drop account: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt drop account: {e}")))?;
    Ok(())
}
