//! Newline-delimited JSON-RPC over stdin/stdout.
//!
//! This is what Claude Code, Windsurf and Cursor use when they spawn the
//! MCP server as a child process. One JSON value per line.

use crate::server::{Outbound, Server};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn run(server: Arc<Server>) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let stdout = Arc::new(tokio::sync::Mutex::new(stdout));

    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Outbound>();

    // Writer task — single owner of stdout.
    let writer_stdout = stdout.clone();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let line = match msg {
                Outbound::Response(r) => serde_json::to_string(&r),
                Outbound::Notification(v) => serde_json::to_string(&v),
            };
            let line = match line {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "encode outbound");
                    continue;
                }
            };
            let mut out = writer_stdout.lock().await;
            if out.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if out.write_all(b"\n").await.is_err() {
                break;
            }
            if out.flush().await.is_err() {
                break;
            }
        }
    });

    // Reader loop.
    while let Some(line) = reader.next_line().await.transpose() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(error = %e, "stdin read");
                break;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, line = %line, "invalid json on stdin");
                continue;
            }
        };
        server.clone().dispatch(parsed, out_tx.clone()).await;
    }

    drop(out_tx);
    let _ = writer.await;
    Ok(())
}
