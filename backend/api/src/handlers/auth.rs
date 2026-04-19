//! Auth endpoints: register, login, refresh, logout.
//!
//! Token strategy:
//!   * access — JWT (HS256), 15 min, carries `sub` + `role`
//!   * refresh — opaque 32-byte random, 30 d, stored as HMAC-SHA256 hash,
//!     rotated on every /auth/refresh call. Reusing a revoked refresh token
//!     triggers a full revocation of the user's other sessions.

use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use listenai_core::crypto;
use listenai_core::domain::{User, UserRole, UserTier};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use surrealdb::sql::Thing;
use tracing::warn;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::tokens::issue_access_token;
use crate::error::ApiResult;
use crate::state::AppState;

// -------------------------------------------------------------------------
// Request / response DTOs
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RegisterRequest {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 8, max = 128))]
    pub password: String,
    #[validate(length(min = 1, max = 80))]
    pub display_name: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct LoginRequest {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 1, max = 128))]
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LogoutRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_in: i64,
    pub user: User,
}

// -------------------------------------------------------------------------
// Internal row types (private to this module)
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DbUser {
    id: Thing,
    email: String,
    display_name: String,
    role: String,
    tier: String,
    password_hash: Option<String>,
    email_verified_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl DbUser {
    fn id_string(&self) -> String {
        self.id.id.to_raw()
    }

    fn to_domain(&self) -> Result<User> {
        let role = match self.role.as_str() {
            "admin" => UserRole::Admin,
            "user" => UserRole::User,
            other => return Err(Error::Database(format!("unknown role `{other}`"))),
        };
        let tier = match self.tier.as_str() {
            "pro" => UserTier::Pro,
            "free" => UserTier::Free,
            other => return Err(Error::Database(format!("unknown tier `{other}`"))),
        };
        Ok(User {
            id: UserId(self.id_string()),
            email: self.email.clone(),
            display_name: self.display_name.clone(),
            role,
            tier,
            email_verified_at: self.email_verified_at,
            created_at: self.created_at,
        })
    }
}

#[derive(Debug, Deserialize)]
struct DbSession {
    id: Thing,
    user: Thing,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}

// -------------------------------------------------------------------------
// POST /auth/register
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/auth/register",
    tag = "auth",
    request_body = RegisterRequest,
    responses(
        (status = 200, description = "User created + token pair", body = AuthResponse),
        (status = 400, description = "Validation failed"),
        (status = 409, description = "Email already in use")
    )
)]
pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> ApiResult<Json<AuthResponse>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    // Email uniqueness check up front. The DB also enforces via unique index,
    // but this gives a nicer 409 instead of a 500.
    if email_exists(&state, &body.email).await? {
        return Err(Error::Conflict("email already in use".into()).into());
    }

    let password_hash =
        crypto::hash_password(&body.password, state.config().password_pepper.as_bytes())?;
    let id = Uuid::new_v4().simple().to_string();

    let sql = format!(
        r#"CREATE user:`{id}` CONTENT {{
            email: $email,
            display_name: $display_name,
            password_hash: $password_hash,
            role: "user",
            tier: "free"
        }} RETURN AFTER"#
    );
    let created: Vec<DbUser> = state
        .db()
        .inner()
        .query(&sql)
        .bind(("email", body.email.trim().to_lowercase()))
        .bind(("display_name", body.display_name.trim().to_string()))
        .bind(("password_hash", password_hash))
        .await
        .map_err(|e| Error::Database(format!("create user: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("create user (decode): {e}")))?;
    let user = created
        .into_iter()
        .next()
        .ok_or_else(|| Error::Database("create user returned no row".into()))?;

    issue_tokens(&state, &user).await
}

// -------------------------------------------------------------------------
// POST /auth/login
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Logged in", body = AuthResponse),
        (status = 401, description = "Invalid credentials")
    )
)]
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    let email = body.email.trim().to_lowercase();
    let rows: Vec<DbUser> = state
        .db()
        .inner()
        .query("SELECT * FROM user WHERE email = $email LIMIT 1")
        .bind(("email", email))
        .await
        .map_err(|e| Error::Database(format!("lookup user: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("lookup user (decode): {e}")))?;
    let user = rows.into_iter().next().ok_or(Error::Unauthorized)?;

    let Some(hash) = user.password_hash.as_deref() else {
        // No local password (e.g. OAuth-only account in a future phase).
        return Err(Error::Unauthorized.into());
    };
    if !crypto::verify_password(
        &body.password,
        hash,
        state.config().password_pepper.as_bytes(),
    )? {
        return Err(Error::Unauthorized.into());
    }

    issue_tokens(&state, &user).await
}

// -------------------------------------------------------------------------
// POST /auth/refresh
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/auth/refresh",
    tag = "auth",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "New token pair", body = AuthResponse),
        (status = 401, description = "Invalid, expired, or revoked refresh token")
    )
)]
pub async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> ApiResult<Json<AuthResponse>> {
    let hash = crypto::hash_refresh_token(
        &body.refresh_token,
        state.config().password_pepper.as_bytes(),
    )?;

    // Find the session for this refresh-token hash.
    let rows: Vec<DbSession> = state
        .db()
        .inner()
        .query("SELECT * FROM session WHERE refresh_token_hash = $h LIMIT 1")
        .bind(("h", hash.clone()))
        .await
        .map_err(|e| Error::Database(format!("lookup session: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("lookup session (decode): {e}")))?;
    let session = rows.into_iter().next().ok_or(Error::Unauthorized)?;

    // Already revoked = reuse-after-rotation. Nuke the user's sessions.
    if session.revoked_at.is_some() {
        warn!(
            user = %session.user.id.to_raw(),
            "refresh-token reuse detected; revoking all sessions for user"
        );
        revoke_all_user_sessions(&state, &session.user).await?;
        return Err(Error::Unauthorized.into());
    }

    if session.expires_at < Utc::now() {
        return Err(Error::Unauthorized.into());
    }

    // Rotate: mark this session revoked, then issue fresh tokens.
    state
        .db()
        .inner()
        .query("UPDATE $id SET revoked_at = time::now()")
        .bind(("id", session.id.clone()))
        .await
        .map_err(|e| Error::Database(format!("revoke session: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("revoke session: {e}")))?;

    // Load the user to produce a fresh access token + response body.
    let user_id = session.user.id.to_raw();
    let user_rows: Vec<DbUser> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM user:`{user_id}`"))
        .await
        .map_err(|e| Error::Database(format!("lookup user on refresh: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("lookup user on refresh (decode): {e}")))?;
    let user = user_rows.into_iter().next().ok_or(Error::Unauthorized)?;

    issue_tokens(&state, &user).await
}

// -------------------------------------------------------------------------
// POST /auth/logout
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/auth/logout",
    tag = "auth",
    request_body = LogoutRequest,
    responses((status = 204, description = "Logged out"))
)]
pub async fn logout(
    State(state): State<AppState>,
    Json(body): Json<LogoutRequest>,
) -> ApiResult<axum::http::StatusCode> {
    let hash = crypto::hash_refresh_token(
        &body.refresh_token,
        state.config().password_pepper.as_bytes(),
    )?;
    state
        .db()
        .inner()
        .query("UPDATE session SET revoked_at = time::now() WHERE refresh_token_hash = $h AND revoked_at IS NONE")
        .bind(("h", hash))
        .await
        .map_err(|e| Error::Database(format!("logout: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("logout: {e}")))?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// -------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------

async fn email_exists(state: &AppState, email: &str) -> Result<bool> {
    // Use `VALUE email` so the result set is a plain Vec<String> — the
    // record-id Thing type does not round-trip through serde_json::Value.
    let rows: Vec<String> = state
        .db()
        .inner()
        .query("SELECT VALUE email FROM user WHERE email = $email LIMIT 1")
        .bind(("email", email.trim().to_lowercase()))
        .await
        .map_err(|e| Error::Database(format!("email check: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("email check (decode): {e}")))?;
    Ok(!rows.is_empty())
}

async fn issue_tokens(state: &AppState, user: &DbUser) -> ApiResult<Json<AuthResponse>> {
    let domain_user = user.to_domain()?;
    let access_token = issue_access_token(
        &domain_user.id,
        domain_user.role,
        &state.config().jwt_secret,
        state.config().access_token_ttl(),
    )?;

    let refresh_raw = crypto::new_refresh_token();
    let refresh_hash =
        crypto::hash_refresh_token(&refresh_raw, state.config().password_pepper.as_bytes())?;

    let session_id = Uuid::new_v4().simple().to_string();
    let user_thing = user.id.clone();
    // Let SurrealDB compute the expiry with its own datetime type — passing
    // a bound `DateTime<Utc>` serializes as a string and the schema rejects
    // it.
    let ttl_secs = state.config().refresh_token_ttl_secs;
    let sql = format!(
        r#"CREATE session:`{session_id}` CONTENT {{
            user: $user,
            refresh_token_hash: $hash,
            expires_at: time::now() + <duration> "{ttl_secs}s"
        }}"#
    );
    state
        .db()
        .inner()
        .query(&sql)
        .bind(("user", user_thing))
        .bind(("hash", refresh_hash))
        .await
        .map_err(|e| Error::Database(format!("create session: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create session: {e}")))?;

    Ok(Json(AuthResponse {
        access_token,
        refresh_token: refresh_raw,
        access_token_expires_in: state.config().access_token_ttl_secs as i64,
        user: domain_user,
    }))
}

async fn revoke_all_user_sessions(state: &AppState, user_thing: &Thing) -> Result<()> {
    state
        .db()
        .inner()
        .query(
            "UPDATE session SET revoked_at = time::now() WHERE user = $user AND revoked_at IS NONE",
        )
        .bind(("user", user_thing.clone()))
        .await
        .map_err(|e| Error::Database(format!("revoke user sessions: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("revoke user sessions: {e}")))?;
    Ok(())
}

// Silences `unused` if we later add fields to AuthResponse we build
// elsewhere.
#[allow(dead_code)]
fn _ensure_json_used() -> serde_json::Value {
    json!({})
}
