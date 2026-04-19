//! Current-user introspection and profile updates.

use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use listenai_core::domain::{User, UserRole, UserTier};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use utoipa::ToSchema;
use validator::Validate;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    pub user: User,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateMeRequest {
    #[validate(length(min = 1, max = 80))]
    pub display_name: Option<String>,
}

#[utoipa::path(
    get,
    path = "/me",
    tag = "auth",
    responses(
        (status = 200, description = "The authenticated user", body = MeResponse),
        (status = 401, description = "Missing or invalid access token")
    ),
    security(("bearer" = []))
)]
pub async fn get_me(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<MeResponse>> {
    let row = load_user(&state, &user.id).await?;
    Ok(Json(MeResponse {
        user: row.to_domain()?,
    }))
}

#[utoipa::path(
    patch,
    path = "/me",
    tag = "auth",
    request_body = UpdateMeRequest,
    responses(
        (status = 200, description = "The updated user", body = MeResponse),
        (status = 400, description = "Validation failed"),
        (status = 401, description = "Missing or invalid access token")
    ),
    security(("bearer" = []))
)]
pub async fn patch_me(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Json(body): Json<UpdateMeRequest>,
) -> ApiResult<Json<MeResponse>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    if let Some(name) = body.display_name.as_deref() {
        let id = user.id.0.clone();
        state
            .db()
            .inner()
            .query(format!("UPDATE user:`{id}` SET display_name = $name"))
            .bind(("name", name.trim().to_string()))
            .await
            .map_err(|e| Error::Database(format!("update /me: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("update /me: {e}")))?;
    }

    let row = load_user(&state, &user.id).await?;
    Ok(Json(MeResponse {
        user: row.to_domain()?,
    }))
}

// -------------------------------------------------------------------------
// Internal
// -------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct DbUser {
    id: Thing,
    email: String,
    display_name: String,
    role: String,
    tier: String,
    email_verified_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl DbUser {
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
            id: UserId(self.id.id.to_raw()),
            email: self.email.clone(),
            display_name: self.display_name.clone(),
            role,
            tier,
            email_verified_at: self.email_verified_at,
            created_at: self.created_at,
        })
    }
}

async fn load_user(state: &AppState, id: &UserId) -> Result<DbUser> {
    let raw = id.0.clone();
    let rows: Vec<DbUser> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM user:`{raw}`"))
        .await
        .map_err(|e| Error::Database(format!("load user: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load user (decode): {e}")))?;
    rows.into_iter().next().ok_or_else(|| Error::NotFound {
        resource: format!("user:{raw}"),
    })
}
