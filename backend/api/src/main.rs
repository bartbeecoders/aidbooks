#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod app;
mod auth;
mod error;
mod generation;
mod handlers;
mod llm;
mod openapi;
mod state;

use std::process::ExitCode;

use listenai_core::config::{Config, LogFormat};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    // Load config before we init tracing so log level/format are respected.
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(78);
        }
    };
    init_tracing(&config);

    // Per SurrealDB performance guidance: multi-threaded runtime with an
    // enlarged stack (RocksDB + SurrealDB are stack-hungry).
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(10 * 1024 * 1024)
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "failed to build tokio runtime");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = ?e, "fatal");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config) -> anyhow::Result<()> {
    // Storage dirs.
    std::fs::create_dir_all(&config.storage_path)?;

    // Open embedded SurrealDB and converge schema + seeds.
    let db = listenai_db::Db::open(&config.database_path).await?;
    listenai_db::migrate::run(&db).await?;
    listenai_db::seed::run(&db, config.dev_seed, &config.password_pepper).await?;

    let llm = llm::LlmClient::new(
        &config.openrouter_api_key,
        &config.openrouter_base_url,
        config.openrouter_request_timeout_secs,
    )?;
    if llm.is_mock() {
        tracing::warn!(
            "LLM MOCK MODE: openrouter_api_key is empty — all LLM calls return fabricated content"
        );
    }

    let app_state = state::AppState::new(config.clone(), db, llm);
    let router = app::build_router(app_state);

    let addr: std::net::SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        %addr,
        version = env!("CARGO_PKG_VERSION"),
        "listenai api listening"
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn init_tracing(config: &Config) {
    let filter = EnvFilter::try_new(&config.log)
        .unwrap_or_else(|_| EnvFilter::new("listenai=debug,tower_http=debug,info"));
    let registry = tracing_subscriber::registry().with(filter);
    match config.log_format {
        LogFormat::Json => registry
            .with(
                fmt::layer()
                    .json()
                    .with_target(true)
                    .with_current_span(true),
            )
            .init(),
        LogFormat::Pretty => registry.with(fmt::layer().with_target(true)).init(),
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
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
