#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod app;
mod audio;
mod auth;
mod error;
mod generation;
mod handlers;
mod i18n;
mod idempotency;
mod jobs;
mod llm;
mod openapi;
mod state;
mod tts;

use std::process::ExitCode;
use std::time::Duration;

use listenai_core::config::{Config, LogFormat};
use listenai_core::domain::JobKind;
use listenai_jobs::{repo::EnqueueRequest, runtime, JobContext, JobRepo, ProgressHub, WorkerConfig};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Nightly cadence for enqueuing the orphan-audio GC sweep. Kept tight in
/// dev (24 h); a prod override can come from config later.
const GC_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

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

    let tts = tts::build(
        &config.xai_api_key,
        &config.xai_tts_url,
        config.xai_sample_rate_hz,
        config.xai_request_timeout_secs,
    );
    if tts.is_mock() {
        tracing::warn!(
            "TTS MOCK MODE: xai_api_key is empty — all TTS calls return a low-amplitude tone"
        );
    }

    // Phase 5: job infra.
    let job_repo = JobRepo::new(db.clone());
    let hub = ProgressHub::new();

    let app_state = state::AppState::new(
        config.clone(),
        db,
        llm,
        tts,
        job_repo.clone(),
        hub.clone(),
    );

    let registry = jobs::registry(app_state.clone());
    let worker_ctx = JobContext::new(job_repo.clone(), hub.clone());
    let worker_handle =
        runtime::spawn(worker_ctx, registry, WorkerConfig::default()).await;
    let gc_scheduler = spawn_gc_scheduler(job_repo.clone());

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

    tracing::info!("draining jobs...");
    gc_scheduler.abort();
    worker_handle.shutdown().await;
    Ok(())
}

/// Periodic task that enqueues one GC job every `GC_INTERVAL`. The first
/// tick fires after the interval — the startup path is deliberately clean.
fn spawn_gc_scheduler(repo: JobRepo) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(GC_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the immediate first tick — we want to enqueue on the
        // schedule, not on boot.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let req = EnqueueRequest::new(JobKind::Gc).with_max_attempts(1);
            if let Err(e) = repo.enqueue(req).await {
                tracing::warn!(error = %e, "gc enqueue failed");
            } else {
                tracing::info!("gc enqueued");
            }
        }
    })
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
