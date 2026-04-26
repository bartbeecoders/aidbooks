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
        // --- Phase 3: content generation ---
        .route(
            "/audiobook",
            post(handlers::audiobook::create).get(handlers::audiobook::list),
        )
        .route(
            "/audiobook/:id",
            get(handlers::audiobook::get_one)
                .patch(handlers::audiobook::patch)
                .delete(handlers::audiobook::delete),
        )
        .route(
            "/audiobook/:id/generate-chapters",
            post(handlers::audiobook::generate_chapters),
        )
        .route(
            "/audiobook/:id/chapter/:n",
            axum::routing::patch(handlers::audiobook::patch_chapter),
        )
        .route(
            "/audiobook/:id/chapter/:n/regenerate",
            post(handlers::audiobook::regenerate_chapter),
        )
        // --- Phase 4: audio generation + streaming ---
        .route(
            "/audiobook/:id/generate-audio",
            post(handlers::audiobook::generate_audio),
        )
        .route(
            "/audiobook/:id/chapter/:n/regenerate-audio",
            post(handlers::audiobook::regenerate_chapter_audio),
        )
        .route(
            "/audiobook/:id/chapter/:n/audio",
            get(handlers::stream::chapter_audio),
        )
        .route(
            "/audiobook/:id/chapter/:n/waveform",
            get(handlers::stream::chapter_waveform),
        )
        // --- Phase 6: cover art ---
        .route("/cover-art/preview", post(handlers::cover::preview))
        .route(
            "/audiobook/:id/cover",
            get(handlers::stream::cover).post(handlers::audiobook::regenerate_cover),
        )
        .route(
            "/audiobook/:id/translate",
            post(handlers::audiobook::translate),
        )
        // --- Phase 5: jobs + real-time progress ---
        .route(
            "/audiobook/:id/jobs",
            get(handlers::jobs::list_for_audiobook),
        )
        .route(
            "/ws/audiobook/:id",
            get(handlers::ws::audiobook_progress),
        )
        .route("/topics/random", post(handlers::topics::random))
        .route("/voices", get(handlers::catalog::list_voices))
        .route("/llms", get(handlers::catalog::list_llms))
        // --- Phase 7: admin ---
        .route("/admin/system", get(handlers::admin::system_overview))
        .route(
            "/admin/llm",
            get(handlers::admin::list_llms).post(handlers::admin::create_llm),
        )
        .route(
            "/admin/llm/:id",
            axum::routing::patch(handlers::admin::patch_llm),
        )
        .route("/admin/voice", get(handlers::admin::list_voices))
        .route(
            "/admin/voice/:id",
            axum::routing::patch(handlers::admin::patch_voice),
        )
        .route("/admin/users", get(handlers::admin::list_users))
        .route(
            "/admin/users/:id",
            axum::routing::patch(handlers::admin::patch_user),
        )
        .route(
            "/admin/users/:id/revoke-sessions",
            post(handlers::admin::revoke_sessions),
        )
        .route("/admin/jobs", get(handlers::admin::list_jobs))
        .route(
            "/admin/jobs/:id/retry",
            post(handlers::admin::retry_job),
        )
        .route("/admin/test/llm", post(handlers::admin::test_llm))
        .route("/admin/test/voice", post(handlers::admin::test_voice))
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
            HeaderName::from_static("idempotency-key"),
        ])
        .expose_headers([REQUEST_ID_HEADER.clone()])
        .allow_credentials(true)
        .max_age(Duration::from_secs(600))
}
