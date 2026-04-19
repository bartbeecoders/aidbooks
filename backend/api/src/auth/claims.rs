use listenai_core::domain::UserRole;
use listenai_core::id::UserId;
use serde::{Deserialize, Serialize};

/// Payload of an access-token JWT. Kept small — the refresh path and /me
/// endpoint do the heavy lookups if we need more than role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    /// Subject — user id.
    pub sub: UserId,
    pub role: UserRole,
    /// Issued-at epoch seconds.
    pub iat: i64,
    /// Expiry epoch seconds.
    pub exp: i64,
    /// Unique token id.
    pub jti: String,
}

/// The summary of an authenticated user, attached to request extensions by
/// the [`crate::auth::Authenticated`] extractor.
#[derive(Debug, Clone)]
pub struct AuthedUser {
    pub id: UserId,
    // Only read by `RequireAdmin` (Phase 7). Silences dead-code until then.
    #[allow(dead_code)]
    pub role: UserRole,
}
