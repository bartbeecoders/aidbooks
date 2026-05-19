use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::handlers;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(handlers::healthz))
        .route("/v1/defaults", get(handlers::defaults))
        .route("/v1/generate", post(handlers::generate))
        .route("/v1/models/pull", post(handlers::pull))
        .route("/v1/models/unload", delete(handlers::unload))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn run(config: Config) -> anyhow::Result<()> {
    let addr = config.addr();
    let auth_required = config.api_key.is_some();
    tracing::info!(
        addr = %addr,
        upstream = %config.upstream_url,
        max_concurrency = config.max_concurrency,
        timeout_secs = config.timeout_secs,
        auth_required,
        "starting mold-service"
    );
    let state = AppState::new(config);
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
