#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod config;
mod http_client;
mod proto;
mod server;
mod tools;
mod transport;

use std::process::ExitCode;
use std::sync::Arc;

use config::{Config, Transport};
use http_client::ApiClient;
use server::Server;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    init_tracing();

    let cfg = match Config::from_args_and_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(78);
        }
    };

    // Multi-thread runtime — the api proxy + WS subscriber both want async I/O.
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "tokio runtime");
            return ExitCode::FAILURE;
        }
    };

    match rt.block_on(run(cfg)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = ?e, "fatal");
            ExitCode::FAILURE
        }
    }
}

async fn run(cfg: Config) -> anyhow::Result<()> {
    let client = Arc::new(ApiClient::new(
        &cfg.api_base_url,
        cfg.default_token.clone(),
        cfg.request_timeout_secs,
    )?);

    let registry = tools::build_registry(client.clone(), cfg.api_ws_url.clone()).await?;
    tracing::info!(
        api = %cfg.api_base_url,
        ws = %cfg.api_ws_url,
        tools = registry.len(),
        "ready",
    );
    let server = Arc::new(Server::new(Arc::new(registry)));

    match cfg.transport {
        Transport::Stdio => transport::stdio::run(server).await,
        Transport::Http => transport::http::run(server, &cfg.http_bind).await,
    }
}

fn init_tracing() {
    // For stdio mode we MUST log to stderr — stdout is the protocol channel.
    let filter = EnvFilter::try_from_env("LISTENAI_MCP_LOG")
        .unwrap_or_else(|_| EnvFilter::new("listenai_mcp=info,info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_target(false))
        .init();
}
