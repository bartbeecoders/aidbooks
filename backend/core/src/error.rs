//! Core error enum shared across layers. The API crate maps this (plus its
//! own HTTP-specific variants) into wire-format `IntoResponse` bodies.

use serde::Serialize;
use thiserror::Error;
use utoipa::ToSchema;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not found: {resource}")]
    NotFound { resource: String },

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("rate limited")]
    RateLimited,

    #[error("upstream service error: {0}")]
    Upstream(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// Stable, machine-readable error code for API responses.
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound { .. } => "not_found",
            Error::Validation(_) => "validation_failed",
            Error::Unauthorized => "unauthorized",
            Error::Forbidden => "forbidden",
            Error::Conflict(_) => "conflict",
            Error::RateLimited => "rate_limited",
            Error::Upstream(_) => "upstream_error",
            Error::Database(_) => "database_error",
            Error::Config(_) => "config_error",
            Error::Other(_) => "internal_error",
        }
    }
}

/// Wire-format error body. Returned by the API layer for every non-2xx
/// response. `request_id` is populated from the `x-request-id` header.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub request_id: Option<String>,
}
