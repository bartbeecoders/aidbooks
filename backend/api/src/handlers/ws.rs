//! WebSocket progress stream for a single audiobook.
//!
//! Auth: browser `WebSocket` constructors can't send custom headers, so we
//! additionally accept the access token as `?access_token=…`. The Authorization
//! header is honoured too (curl, Swagger UI, mobile).
//!
//! On connect we send one `snapshot` event (latest DB state of every job for
//! the book) so the UI renders without a REST round-trip, then forward live
//! events from the in-memory broadcast hub until the client disconnects.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{header, HeaderMap},
    response::IntoResponse,
};
use chrono::Utc;
use listenai_core::id::{AudiobookId, UserId};
use listenai_core::{Error, Result};
use listenai_jobs::hub::{JobSnapshot, ProgressEvent};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use crate::auth::tokens::verify_access_token;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    /// Browser-friendly fallback: most JS WebSocket clients can't send
    /// custom headers, so we also accept the bearer token as a query param.
    #[serde(default)]
    pub access_token: Option<String>,
}

pub async fn audiobook_progress(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<WsQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<impl IntoResponse> {
    let user_id = authenticate(&state, &headers, &q).await?;
    assert_owner(&state, &id, &user_id).await?;

    let state_clone = state.clone();
    let audiobook_id = id.clone();
    Ok(ws.on_upgrade(move |socket| async move {
        if let Err(e) = serve(socket, state_clone, audiobook_id).await {
            debug!(error = %e, "ws closed with error");
        }
    }))
}

async fn serve(mut socket: WebSocket, state: AppState, audiobook_id: String) -> Result<()> {
    // Subscribe before the snapshot so we don't miss events that fire
    // between snapshot load and first receive.
    let aid = AudiobookId(audiobook_id.clone());
    let mut rx = state.hub().subscribe(&aid).await;

    // Send snapshot. Any error here is terminal for this connection.
    let snapshot = load_snapshot(&state, &audiobook_id).await?;
    let snapshot_json = serde_json::to_string(&snapshot)
        .map_err(|e| Error::Database(format!("snapshot encode: {e}")))?;
    if socket.send(Message::Text(snapshot_json)).await.is_err() {
        return Ok(());
    }

    // Forward events until the client disconnects.
    loop {
        tokio::select! {
            recv = rx.recv() => match recv {
                Ok(event) => {
                    let text = match serde_json::to_string(&event) {
                        Ok(t) => t,
                        Err(e) => {
                            warn!(error = %e, "event encode failed; dropping");
                            continue;
                        }
                    };
                    if socket.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    warn!(skipped = n, "ws subscriber lagged");
                    // Re-snapshot so the UI catches up with DB truth.
                    if let Ok(snap) = load_snapshot(&state, &audiobook_id).await {
                        if let Ok(text) = serde_json::to_string(&snap) {
                            if socket.send(Message::Text(text)).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                Err(RecvError::Closed) => break,
            },
            client = socket.recv() => match client {
                Some(Ok(Message::Ping(b))) => {
                    let _ = socket.send(Message::Pong(b)).await;
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                _ => {}
            }
        }
    }

    // Drop the receiver so the hub can GC an idle book channel.
    drop(rx);
    state.hub().gc(&audiobook_id).await;
    Ok(())
}

async fn load_snapshot(state: &AppState, audiobook_id: &str) -> Result<ProgressEvent> {
    let jobs = state.jobs().list_for_audiobook(audiobook_id).await?;
    let snapshots = jobs
        .into_iter()
        .map(|j| JobSnapshot {
            id: j.id,
            kind: j.kind.as_str().to_string(),
            status: j.status.as_str().to_string(),
            progress_pct: j.progress_pct,
            attempts: j.attempts,
            chapter_number: j.chapter_number,
            last_error: j.last_error,
        })
        .collect();
    Ok(ProgressEvent::Snapshot {
        audiobook_id: audiobook_id.to_string(),
        jobs: snapshots,
        at: Utc::now(),
    })
}

async fn authenticate(state: &AppState, headers: &HeaderMap, q: &WsQuery) -> Result<UserId> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| {
            h.strip_prefix("Bearer ")
                .or_else(|| h.strip_prefix("bearer "))
        })
        .map(str::to_string)
        .or_else(|| q.access_token.clone())
        .ok_or(Error::Unauthorized)?;

    let claims = verify_access_token(&token, &state.config().jwt_secret)?;
    Ok(claims.sub)
}

async fn assert_owner(state: &AppState, audiobook_id: &str, user: &UserId) -> Result<()> {
    #[derive(Deserialize)]
    struct Row {
        owner: surrealdb::sql::Thing,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!("SELECT owner FROM audiobook:`{audiobook_id}`"))
        .await
        .map_err(|e| Error::Database(format!("ws owner: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("ws owner (decode): {e}")))?;
    let owner = rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{audiobook_id}"),
    })?;
    if owner.owner.id.to_raw() != user.0 {
        // Opaque 404 consistent with the REST endpoints — never leaks
        // existence to a non-owner.
        return Err(Error::NotFound {
            resource: format!("audiobook:{audiobook_id}"),
        });
    }
    Ok(())
}
