use std::time::Duration;

use axum::{
    extract::MatchedPath,
    http::{HeaderName, HeaderValue, Method, Request},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use http::{header, StatusCode};
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use utoipa::OpenApi;

use crate::{handlers, openapi::ApiDoc, state::AppState};

const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

pub fn build_router(state: AppState) -> Router {
    let cors = build_cors(&state);
    let timeout = Duration::from_secs(state.config().request_timeout_secs);

    let middleware = ServiceBuilder::new()
        // Assign a request id to every incoming request that doesn't already
        // carry one, then propagate it into the response headers.
        .layer(SetRequestIdLayer::new(
            REQUEST_ID_HEADER.clone(),
            MakeRequestUuid,
        ))
        .layer(PropagateRequestIdLayer::new(REQUEST_ID_HEADER.clone()))
        // Structured request/response logs. The request-id is added to the
        // span so every log line within a request is correlatable.
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &Request<_>| {
                let route = req
                    .extensions()
                    .get::<MatchedPath>()
                    .map(|p| p.as_str())
                    .unwrap_or("");
                let request_id = req
                    .headers()
                    .get(&REQUEST_ID_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                tracing::info_span!(
                    "http",
                    method = %req.method(),
                    route = %route,
                    uri = %req.uri(),
                    request_id = %request_id,
                )
            }),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            timeout,
        ))
        .layer(CompressionLayer::new())
        .layer(cors);

    Router::new()
        .route("/health", get(handlers::health::health))
        .route("/ready", get(handlers::health::ready))
        .route("/openapi.json", get(openapi_json))
        // --- Phase 2: auth ---
        .route("/auth/register", post(handlers::auth::register))
        .route("/auth/login", post(handlers::auth::login))
        .route("/auth/refresh", post(handlers::auth::refresh))
        .route("/auth/logout", post(handlers::auth::logout))
        .route(
            "/me",
            get(handlers::me::get_me).patch(handlers::me::patch_me),
        )
        .fallback(not_found)
        .with_state(state)
        .layer(middleware)
}

async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}

async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "code": "not_found",
            "message": "route not found",
        })),
    )
}

fn build_cors(state: &AppState) -> CorsLayer {
    let origins: Vec<HeaderValue> = state
        .config()
        .cors_allow_origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            REQUEST_ID_HEADER.clone(),
        ])
        .expose_headers([REQUEST_ID_HEADER.clone()])
        .allow_credentials(true)
        .max_age(Duration::from_secs(600))
}
