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
            "/audiobook/:id/animate",
            post(handlers::audiobook::animate),
        )
        .route(
            "/audiobook/:id/chapter/:n/animate",
            post(handlers::audiobook::animate_chapter),
        )
        .route(
            "/audiobook/:id/cancel-pipeline",
            post(handlers::audiobook::cancel_pipeline),
        )
        .route(
            "/audiobook/:id/chapter/:n/regenerate-audio",
            post(handlers::audiobook::regenerate_chapter_audio),
        )
        .route(
            "/audiobook/:id/chapter/:n/classify-visuals",
            post(handlers::audiobook::classify_chapter_visuals),
        )
        .route(
            "/audiobook/:id/chapter/:n/regenerate-manim-code",
            post(handlers::audiobook::regenerate_chapter_manim_code),
        )
        .route(
            "/audiobook/:id/chapter/:n/test-manim-llm",
            post(handlers::audiobook::test_chapter_manim_llm),
        )
        .route(
            "/audiobook/:id/chapter/:n/test-manim-render",
            post(handlers::audiobook::render_test_manim),
        )
        .route(
            "/audiobook/:id/test-manim/:test_id",
            get(handlers::stream::test_manim_video),
        )
        .route(
            "/audiobook/:id/chapter/:n/art",
            get(handlers::stream::chapter_art).post(handlers::audiobook::regenerate_chapter_art),
        )
        .route(
            "/audiobook/:id/chapter/:n/paragraph/:p/image/:i",
            get(handlers::stream::paragraph_image),
        )
        .route(
            "/audiobook/:id/chapter/:n/audio",
            get(handlers::stream::chapter_audio),
        )
        .route(
            "/audiobook/:id/chapter/:n/waveform",
            get(handlers::stream::chapter_waveform),
        )
        .route(
            "/audiobook/:id/chapter/:n/video",
            get(handlers::stream::chapter_video),
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
        .route(
            "/audiobook/:id/costs",
            get(handlers::audiobook::costs),
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
        .route(
            "/topic-templates",
            get(handlers::topic_templates::list_public),
        )
        .route("/voices", get(handlers::catalog::list_voices))
        .route(
            "/voices/:id/preview",
            get(handlers::catalog::preview_voice),
        )
        .route("/llms", get(handlers::catalog::list_llms))
        .route(
            "/audiobook-categories",
            get(handlers::catalog::list_audiobook_categories),
        )
        // --- Phase 7: admin ---
        .route("/admin/system", get(handlers::admin::system_overview))
        .route(
            "/admin/llm",
            get(handlers::admin::list_llms).post(handlers::admin::create_llm),
        )
        .route(
            "/admin/llm/:id",
            axum::routing::patch(handlers::admin::patch_llm)
                .delete(handlers::admin::delete_llm),
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
            "/admin/jobs/:id",
            axum::routing::delete(handlers::admin::delete_job),
        )
        .route(
            "/admin/jobs/:id/retry",
            post(handlers::admin::retry_job),
        )
        .route(
            "/admin/jobs/:id/cancel",
            post(handlers::admin::cancel_job),
        )
        .route("/admin/test/llm", post(handlers::admin::test_llm))
        .route("/admin/test/voice", post(handlers::admin::test_voice))
        .route(
            "/admin/openrouter/models",
            get(handlers::admin::list_openrouter_models),
        )
        .route(
            "/admin/xai/models",
            get(handlers::admin::list_xai_models),
        )
        .route(
            "/admin/xai/image-models",
            get(handlers::admin::list_xai_image_models),
        )
        .route(
            "/admin/youtube-settings",
            get(handlers::admin::list_youtube_footers),
        )
        .route(
            "/admin/youtube-settings/:language",
            axum::routing::put(handlers::admin::upsert_youtube_footer)
                .delete(handlers::admin::delete_youtube_footer),
        )
        .route(
            "/admin/youtube-publish-settings",
            get(handlers::admin::get_youtube_publish_settings)
                .put(handlers::admin::put_youtube_publish_settings),
        )
        .route(
            "/admin/audiobook-categories",
            get(handlers::admin::list_audiobook_categories)
                .post(handlers::admin::create_audiobook_category),
        )
        .route(
            "/admin/audiobook-categories/:id",
            axum::routing::patch(handlers::admin::update_audiobook_category)
                .delete(handlers::admin::delete_audiobook_category),
        )
        .route(
            "/admin/topic-templates",
            get(handlers::topic_templates::list_admin)
                .post(handlers::topic_templates::create),
        )
        .route(
            "/admin/topic-templates/:id",
            axum::routing::patch(handlers::topic_templates::patch)
                .delete(handlers::topic_templates::delete),
        )
        // --- Ideas (audiobook idea backlog + LLM trend suggestions) ---
        .route(
            "/ideas",
            get(handlers::ideas::list).post(handlers::ideas::create),
        )
        .route(
            "/ideas/suggest",
            post(handlers::ideas::suggest),
        )
        .route(
            "/ideas/:id",
            axum::routing::patch(handlers::ideas::patch)
                .delete(handlers::ideas::delete),
        )
        // --- Phase 11: podcasts ---
        .route(
            "/podcasts",
            get(handlers::podcasts::list).post(handlers::podcasts::create),
        )
        .route(
            "/podcasts/preview-image",
            post(handlers::podcasts::preview_image),
        )
        .route(
            "/podcasts/:id",
            get(handlers::podcasts::get_one)
                .patch(handlers::podcasts::patch)
                .delete(handlers::podcasts::delete),
        )
        .route(
            "/podcasts/:id/image",
            get(handlers::podcasts::image),
        )
        .route(
            "/podcasts/:id/sync-youtube",
            post(handlers::podcasts::sync_youtube),
        )
        // --- Phase 8: integrations (YouTube publishing) ---
        .route(
            "/integrations/youtube/oauth/start",
            get(handlers::integrations::youtube_oauth_start),
        )
        .route(
            "/integrations/youtube/oauth/callback",
            get(handlers::integrations::youtube_oauth_callback),
        )
        .route(
            "/integrations/youtube/account",
            get(handlers::integrations::youtube_account_status)
                .delete(handlers::integrations::youtube_account_disconnect),
        )
        .route(
            "/audiobook/:id/publish/youtube",
            post(handlers::integrations::publish_youtube),
        )
        .route(
            "/audiobook/:id/publications",
            get(handlers::integrations::list_publications),
        )
        .route(
            "/audiobook/:id/publications/:pid/approve",
            post(handlers::integrations::approve_publication),
        )
        .route(
            "/audiobook/:id/publications/:pid/cancel",
            post(handlers::integrations::cancel_publication),
        )
        .route(
            "/audiobook/:id/publications/:pid/preview",
            get(handlers::integrations::preview_publication),
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
            HeaderName::from_static("idempotency-key"),
        ])
        .expose_headers([REQUEST_ID_HEADER.clone()])
        .allow_credentials(true)
        .max_age(Duration::from_secs(600))
}
