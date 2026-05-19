//! Shared YouTube-account helpers used by both HTTP handlers and the
//! background publish job.
//!
//! Resolving an access token requires three steps:
//!   1. Look up the `youtube_account` row for the user.
//!   2. Decrypt the stored refresh token with `Config.password_pepper`.
//!   3. Trade the refresh token for a fresh access token (Google's
//!      access tokens are short-lived, so we don't cache them).
//!
//! Why connections still get "lost" intermittently:
//!   * **OAuth app in `Testing` publishing status** at Google Cloud
//!     Console — refresh tokens issued to test users expire **7 days**
//!     after issuance. The fix is to publish the consent screen to
//!     `Production`; nothing we do in code makes this go away.
//!   * **Refresh-token rotation** — when Google decides to rotate, the
//!     refresh response carries a new `refresh_token` and the old one
//!     stops working immediately. We persist any rotated value here so
//!     this isn't a one-shot kill.
//!   * **Refresh-token cap** — Google invalidates the oldest refresh
//!     token once a single (user, client_id) pair issues more than 50.
//!     Reconnecting the same channel repeatedly accelerates this.
//!
//! On any `invalid_grant` from Google we drop the local row immediately
//! so the next status query reflects the disconnect — the user shouldn't
//! have to wait for the next failing upstream call to discover it.

use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use tracing::{info, warn};

use crate::state::AppState;
use crate::youtube::{encrypt, oauth};

/// Decrypt + refresh the user's YouTube refresh token. Returns:
///   * `Ok(Some(token))` — connected and the token is good to use.
///   * `Ok(None)`        — the user has not connected a YouTube channel.
///   * `Err(Unauthorized)` — the user revoked at Google (or the token
///     hit Google's 7-day testing-mode expiry / 50-token cap). The
///     local row is already dropped, so the next status query shows
///     `connected: false` and prompts the user to reconnect.
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
    let resp = match oauth::refresh_access(
        &cfg.youtube_client_id,
        &cfg.youtube_client_secret,
        &refresh,
    )
    .await
    {
        Ok(r) => r,
        Err(Error::Unauthorized) => {
            // invalid_grant — the stored refresh token is permanently
            // dead (revoked, 7-day testing-mode expiry, or the cap of
            // 50 refresh tokens per (user, client_id) rolled it off).
            // Drop the local row up front so the UI surfaces a
            // disconnected state on the next status poll instead of
            // making the user trigger another failing upstream call.
            warn!(
                user = user.0,
                "yt access_token: refresh returned invalid_grant; dropping local row"
            );
            let _ = drop_account(state, user).await;
            return Err(Error::Unauthorized);
        }
        Err(e) => return Err(e),
    };

    // Persist any rotated refresh token. Google does not rotate for
    // installed-app clients by default, but it MAY rotate under some
    // configurations and the old token is then dead immediately. If we
    // don't store the rotated value, the next refresh fails with
    // invalid_grant and the user sees a "connection lost" prompt for
    // no good reason. This update is idempotent and a no-op when
    // Google returned no new refresh token.
    if let Some(new_refresh) = resp.refresh_token.as_deref() {
        if !new_refresh.is_empty() && new_refresh != refresh {
            match encrypt::encrypt(new_refresh, pepper) {
                Ok(new_enc) => {
                    if let Err(e) = update_refresh_token(state, user, &new_enc).await {
                        warn!(
                            error = %e,
                            user = user.0,
                            "yt access_token: failed to persist rotated refresh token; \
                             next refresh may force a reconnect"
                        );
                    } else {
                        info!(user = user.0, "yt access_token: persisted rotated refresh token");
                    }
                }
                Err(e) => warn!(
                    error = %e,
                    user = user.0,
                    "yt access_token: encrypt of rotated refresh token failed"
                ),
            }
        }
    }

    Ok(Some(resp.access_token))
}

/// Overwrite the user's stored refresh-token ciphertext. Used by
/// [`access_token`] when Google rotates the refresh token in a refresh
/// response. We don't touch any other column so the channel binding
/// stays intact.
async fn update_refresh_token(
    state: &AppState,
    user: &UserId,
    refresh_enc: &str,
) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_account SET refresh_token_enc = $enc \
             WHERE owner = user:`{}`",
            user.0
        ))
        .bind(("enc", refresh_enc.to_string()))
        .await
        .map_err(|e| Error::Database(format!("yt update refresh: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt update refresh: {e}")))?;
    Ok(())
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
