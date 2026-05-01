//! Streamable HTTP transport (per MCP 2025-06-18 spec).
//!
//! A single endpoint, `POST /mcp`. The body is a JSON-RPC request (or batch).
//! Responses are returned either:
//!   * as `application/json` for short-lived calls (the default), or
//!   * as `text/event-stream` (SSE) when the client sets
//!     `Accept: text/event-stream`. SSE lets us push intermediate
//!     `notifications/progress` events for long-running tools like
//!     `audiobook_subscribe_progress`.
//!
//! `GET /mcp` returns server-info as JSON for liveness probes.

use crate::server::{Outbound, Server};
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response as AxumResponse},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::{wrappers::UnboundedReceiverStream, StreamExt};

#[derive(Clone)]
struct AppState {
    server: Arc<Server>,
}

pub async fn run(server: Arc<Server>, bind: &str) -> anyhow::Result<()> {
    let state = AppState { server };
    let router = Router::new()
        .route("/mcp", post(post_mcp).get(get_mcp))
        .route("/health", get(|| async { "ok" }))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "mcp http listener up at /mcp");
    axum::serve(listener, router).await?;
    Ok(())
}

async fn get_mcp() -> impl IntoResponse {
    Json(json!({
        "name": "listenai-mcp",
        "transport": "streamable-http",
        "endpoint": "POST /mcp"
    }))
}

async fn post_mcp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AxumResponse {
    let wants_sse = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("text/event-stream"))
        .unwrap_or(false);

    let (out_tx, out_rx) = mpsc::unbounded_channel::<Outbound>();
    let server = state.server.clone();
    // dispatch returns when the per-message tasks are SPAWNED, not finished —
    // the receiver stays alive until all spawned tasks drop their senders.
    server.dispatch(body, out_tx).await;

    if wants_sse {
        let stream = UnboundedReceiverStream::new(out_rx).map(|msg| {
            let line = match msg {
                Outbound::Response(r) => serde_json::to_string(&r).unwrap_or_default(),
                Outbound::Notification(v) => serde_json::to_string(&v).unwrap_or_default(),
            };
            Ok::<bytes::Bytes, std::convert::Infallible>(bytes::Bytes::from(format!(
                "event: message\ndata: {line}\n\n"
            )))
        });
        let body = Body::from_stream(stream);
        let mut resp = AxumResponse::new(body);
        *resp.status_mut() = StatusCode::OK;
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        resp
    } else {
        // Drain the channel to completion — the per-message tasks own
        // senders and dropping them ends the stream.
        let mut messages: Vec<Value> = Vec::new();
        let mut stream = UnboundedReceiverStream::new(out_rx);
        while let Some(msg) = stream.next().await {
            match msg {
                Outbound::Response(r) => {
                    if let Ok(v) = serde_json::to_value(r) {
                        messages.push(v);
                    }
                }
                Outbound::Notification(v) => {
                    messages.push(v);
                }
            }
        }
        let body = if messages.len() == 1 {
            messages.into_iter().next().unwrap_or(Value::Null)
        } else {
            Value::Array(messages)
        };
        Json(body).into_response()
    }
}
