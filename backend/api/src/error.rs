//! HTTP wire format for errors. Wraps `listenai_core::Error` with the HTTP
//! status code mapping and request-id propagation.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use listenai_core::error::ErrorBody;
use listenai_core::Error as CoreError;

pub type ApiResult<T> = Result<T, ApiError>;

/// API-layer error. Wraps a `CoreError`. The `x-request-id` is set on the
/// response by the `PropagateRequestIdLayer` middleware rather than here.
#[derive(Debug)]
pub struct ApiError {
    inner: CoreError,
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match &self.inner {
            CoreError::NotFound { .. } => StatusCode::NOT_FOUND,
            CoreError::Validation(_) => StatusCode::BAD_REQUEST,
            CoreError::Unauthorized => StatusCode::UNAUTHORIZED,
            CoreError::Forbidden => StatusCode::FORBIDDEN,
            CoreError::Conflict(_) => StatusCode::CONFLICT,
            CoreError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            CoreError::Upstream(_) => StatusCode::BAD_GATEWAY,
            CoreError::Database(_) | CoreError::Config(_) | CoreError::Other(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl<E: Into<CoreError>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self { inner: e.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!(error = %self.inner, code = self.inner.code(), "server error");
        } else {
            tracing::debug!(error = %self.inner, code = self.inner.code(), "client error");
        }
        let body = ErrorBody {
            code: self.inner.code().to_string(),
            message: self.inner.to_string(),
            request_id: None,
        };
        (status, Json(body)).into_response()
    }
}
