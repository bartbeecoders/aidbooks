//! Request-level idempotency.
//!
//! Clients may include an `Idempotency-Key` header on mutating requests.
//! The first successful request for a given `(user, key)` pair is stored
//! for 24 hours; any repeat within that window returns the cached status
//! and body instead of re-executing the handler.
//!
//! The helper is opt-in: callers invoke [`guard`] at the top of the
//! handler and then either return the cached response or run the work
//! and [`record`] the result. We deliberately do NOT make this a tower
//! middleware — mutating handlers need to make their own choice about
//! which response shape to cache, and a generic body-copy layer would
//! break streaming endpoints without upside.

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, HeaderName},
};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

const HEADER: HeaderName = HeaderName::from_static("idempotency-key");

/// Max header length — keeps malicious clients from filling the DB with a
/// 10-MB "key".
const MAX_KEY_LEN: usize = 255;

/// How long to remember a response. Matches the common industry default.
const TTL_HOURS: i64 = 24;

/// Extractor pulling the `Idempotency-Key` header, if any.
pub struct IdempotencyKey(pub Option<String>);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for IdempotencyKey {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let val = parts.headers.get(&HEADER).and_then(|v| v.to_str().ok());
        let key = match val {
            Some(s) if !s.is_empty() => {
                if s.len() > MAX_KEY_LEN {
                    return Err(Error::Validation(format!(
                        "Idempotency-Key too long (> {MAX_KEY_LEN} bytes)"
                    ))
                    .into());
                }
                Some(s.to_string())
            }
            _ => None,
        };
        Ok(Self(key))
    }
}

/// Cached response body + wire-level status code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cached {
    pub status_code: u16,
    pub body: String,
}

/// Look up a cached response for `(user, key)`. Returns `Ok(None)` if the
/// key is absent or has expired. A stale row is left in place — the GC
/// sweep (or the next write with the same key) cleans it up.
pub async fn lookup(state: &AppState, user: &UserId, key: Option<&str>) -> Result<Option<Cached>> {
    let Some(key) = key else {
        return Ok(None);
    };

    #[derive(Deserialize)]
    struct Row {
        status_code: i64,
        response_body: String,
        expires_at: chrono::DateTime<chrono::Utc>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT status_code, response_body, expires_at FROM request_idempotency \
             WHERE user = user:`{uid}` AND key = $key LIMIT 1",
            uid = user.0,
        ))
        .bind(("key", key.to_string()))
        .await
        .map_err(|e| Error::Database(format!("idempotency lookup: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("idempotency lookup (decode): {e}")))?;

    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    if row.expires_at < chrono::Utc::now() {
        return Ok(None);
    }
    Ok(Some(Cached {
        status_code: row.status_code as u16,
        body: row.response_body,
    }))
}

/// Store a successful response. No-op if `key` is absent. We only record
/// 2xx responses — a failed request should be retryable without hitting
/// the cache.
pub async fn record(
    state: &AppState,
    user: &UserId,
    key: Option<&str>,
    method: &str,
    path: &str,
    status_code: u16,
    body: &str,
) -> Result<()> {
    let Some(key) = key else {
        return Ok(());
    };
    if !(200..300).contains(&status_code) {
        return Ok(());
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    // UPSERT-shaped CREATE with ON DUPLICATE catch: if two concurrent
    // requests race on the same key, the second CREATE trips the UNIQUE
    // index and we silently drop it (the first write wins).
    let sql = format!(
        r#"CREATE request_idempotency:`{id}` CONTENT {{
            user: user:`{user}`,
            key: $key,
            method: $method,
            path: $path,
            status_code: $sc,
            response_body: $body,
            expires_at: time::now() + {TTL_HOURS}h
        }}"#,
        user = user.0,
    );
    let res = state
        .db()
        .inner()
        .query(sql)
        .bind(("key", key.to_string()))
        .bind(("method", method.to_string()))
        .bind(("path", path.to_string()))
        .bind(("sc", status_code as i64))
        .bind(("body", body.to_string()))
        .await;
    match res {
        Ok(r) => {
            // If UNIQUE index trips, `.check()` surfaces it; treat as no-op.
            if let Err(e) = r.check() {
                tracing::debug!(error = %e, "idempotency upsert collided (first-writer wins)");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "idempotency record failed (non-fatal)");
        }
    }
    Ok(())
}
