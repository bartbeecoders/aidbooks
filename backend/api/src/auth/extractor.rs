use async_trait::async_trait;
use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts},
};
use listenai_core::domain::UserRole;
use listenai_core::{Error, Result};

use super::{claims::AuthedUser, tokens::verify_access_token};
use crate::error::ApiError;
use crate::state::AppState;

/// Extractor that passes iff the request carries a valid
/// `Authorization: Bearer <jwt>` header. Attaches the authenticated user.
pub struct Authenticated(pub AuthedUser);

#[async_trait]
impl<S> FromRequestParts<S> for Authenticated
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state: AppState = AppState::from_ref(state);
        let token = bearer_token(parts)?;
        let claims = verify_access_token(&token, &state.config().jwt_secret)?;
        Ok(Self(AuthedUser {
            id: claims.sub,
            role: claims.role,
        }))
    }
}

/// Extractor that passes iff the request is authenticated AND the user has
/// the `admin` role. All other cases collapse to 401/403.
pub struct RequireAdmin(pub AuthedUser);

#[async_trait]
impl<S> FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Authenticated(user) = Authenticated::from_request_parts(parts, state).await?;
        if user.role != UserRole::Admin {
            return Err(Error::Forbidden.into());
        }
        Ok(Self(user))
    }
}

fn bearer_token(parts: &Parts) -> Result<String> {
    let header = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(Error::Unauthorized)?;
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .ok_or(Error::Unauthorized)?;
    if token.is_empty() {
        return Err(Error::Unauthorized);
    }
    Ok(token.to_string())
}
