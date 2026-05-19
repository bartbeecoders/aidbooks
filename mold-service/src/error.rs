use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("upstream mold error: {0}")]
    Upstream(String),
    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Error::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            Error::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            Error::Upstream(m) => (StatusCode::BAD_GATEWAY, m.clone()),
            Error::Internal(e) => {
                tracing::error!(error = %e, "internal mold-service error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
            }
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}

pub type Result<T> = std::result::Result<T, Error>;
