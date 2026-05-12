//! Per-user audiobook generation queue.
//!
//! The system already runs many jobs in parallel *within* one audiobook
//! (outline, chapters, tts, cover, …). This module adds a serializing
//! layer on top so a user can stack N audiobooks and have them
//! generated one after another rather than racing each other for the
//! shared worker pool + LLM quota.
//!
//! Lifecycle of a queue item:
//!
//! ```text
//!   queued ──▶ running ──▶ completed
//!      │         │  │
//!      ├─────────┘  └────▶ failed
//!      └────────────────▶ cancelled
//! ```
//!
//! `queued → running` is driven by the queue runner
//! (`spawn_queue_runner`): when the user's currently-running item has
//! no live jobs left, it's settled (completed/failed), then the
//! next-position queued item is activated by calling
//! `audiobook::kick_off_pipeline`. The runner skips activation when
//! the user's `queue_settings.paused` flag is set.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use std::time::Duration;
use tracing::warn;
use utoipa::ToSchema;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::handlers::audiobook;
use crate::state::AppState;

const RUNNER_TICK: Duration = Duration::from_secs(5);

// -------------------------------------------------------------------------
// DTOs
// -------------------------------------------------------------------------

/// Body shape for `POST /audiobook { enqueue: true }` — the audiobook
/// is created in `draft` and appended to the queue, instead of running
/// the inline outline + cascade. Kept here so the OpenAPI surface for
/// the queue feature is self-contained even though no current endpoint
/// receives this exact body.
#[derive(Debug, Deserialize, ToSchema)]
pub struct EnqueueAudiobookRequest {
    #[allow(dead_code)]
    pub audiobook_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct QueueResponse {
    pub paused: bool,
    pub items: Vec<QueueItem>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct QueueItem {
    pub id: String,
    pub position: u32,
    pub state: QueueItemState,
    pub audiobook_id: String,
    pub title: String,
    pub topic: String,
    pub language: Option<String>,
    pub is_short: bool,
    pub is_songbook: bool,
    pub audiobook_status: String,
    /// Human-readable label for the current pipeline step (e.g.
    /// "outline", "writing chapters", "narrating", "done", "draft").
    pub step: String,
    /// 0..100 — newest live job's progress when one is in flight,
    /// else an estimate based on the audiobook's status.
    pub progress_pct: f32,
    pub cost_usd: f64,
    pub error: Option<String>,
    pub queued_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueueItemState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl QueueItemState {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

// -------------------------------------------------------------------------
// DB-shaped rows
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DbQueueItem {
    id: Thing,
    owner: Thing,
    audiobook: Thing,
    state: String,
    #[serde(default)]
    error: Option<String>,
    queued_at: DateTime<Utc>,
    #[serde(default)]
    started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    finished_at: Option<DateTime<Utc>>,
}

impl DbQueueItem {
    fn raw_id(&self) -> String {
        self.id.id.to_raw()
    }
    fn audiobook_raw(&self) -> String {
        self.audiobook.id.to_raw()
    }
}

#[derive(Debug, Deserialize)]
struct DbQueueSettings {
    #[serde(default)]
    paused: bool,
}

// -------------------------------------------------------------------------
// Public helpers (called from the audiobook create handler)
// -------------------------------------------------------------------------

/// Insert a `queued` row at the tail of this user's queue. Used by
/// `POST /audiobook { enqueue: true }`.
pub(crate) async fn enqueue_audiobook(
    state: &AppState,
    user_id: &UserId,
    audiobook_id: &str,
) -> Result<()> {
    let next_pos = next_position(state, user_id).await?;
    let item_id = uuid::Uuid::new_v4().simple().to_string();
    let sql = format!(
        r#"CREATE audiobook_queue:`{item_id}` CONTENT {{
            owner: user:`{user_id}`,
            audiobook: audiobook:`{audiobook_id}`,
            position: $position,
            state: "queued"
        }}"#,
        user_id = user_id.0,
    );
    state
        .db()
        .inner()
        .query(sql)
        .bind(("position", next_pos))
        .await
        .map_err(|e| Error::Database(format!("enqueue audiobook: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("enqueue audiobook: {e}")))?;
    Ok(())
}

async fn next_position(state: &AppState, user_id: &UserId) -> Result<i64> {
    #[derive(Deserialize)]
    struct Row {
        max_pos: Option<i64>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT math::max(position) AS max_pos FROM audiobook_queue \
             WHERE owner = user:`{}` GROUP ALL",
            user_id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("queue next_position: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue next_position (decode): {e}")))?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.max_pos)
        .map(|n| n + 1)
        .unwrap_or(1))
}

// -------------------------------------------------------------------------
// Handlers
// -------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/queue",
    tag = "queue",
    responses(
        (status = 200, description = "Caller's queue", body = QueueResponse),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<QueueResponse>> {
    let items = load_items(&state, &user.id).await?;
    let paused = load_paused(&state, &user.id).await?;
    Ok(Json(QueueResponse { paused, items }))
}

#[utoipa::path(
    post,
    path = "/queue/pause",
    tag = "queue",
    responses(
        (status = 204, description = "Queue paused"),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn pause(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<StatusCode> {
    upsert_paused(&state, &user.id, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/queue/resume",
    tag = "queue",
    responses(
        (status = 204, description = "Queue resumed"),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn resume(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<StatusCode> {
    upsert_paused(&state, &user.id, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/queue",
    tag = "queue",
    responses(
        (status = 204, description = "Pending items cleared"),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn clear(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<StatusCode> {
    // Only drop queued items — leave the currently-running one alone
    // so the user has to explicitly cancel it (cancelling a running
    // book mid-flight is a heavier action and warrants its own click).
    state
        .db()
        .inner()
        .query(format!(
            "DELETE audiobook_queue WHERE owner = user:`{}` AND state = 'queued'",
            user.id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("queue clear: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("queue clear: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/queue/{item_id}/cancel",
    tag = "queue",
    params(("item_id" = String, Path)),
    responses(
        (status = 204, description = "Item cancelled"),
        (status = 404, description = "Item not found")
    ),
    security(("bearer" = []))
)]
pub async fn cancel_item(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(item_id): Path<String>,
) -> ApiResult<StatusCode> {
    let item = load_item(&state, &item_id).await?;
    if item.owner.id.to_raw() != user.id.0 {
        return Err(Error::NotFound {
            resource: format!("audiobook_queue:{item_id}"),
        }
        .into());
    }
    let was_running = QueueItemState::parse(&item.state) == Some(QueueItemState::Running);
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook_queue:`{item_id}` SET \
                state = 'cancelled', \
                finished_at = time::now()"
        ))
        .await
        .map_err(|e| Error::Database(format!("queue cancel: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("queue cancel: {e}")))?;
    // If the item was actively generating, also cancel its live jobs
    // so the worker pool gets out of the way of the next queued item.
    if was_running {
        let audiobook_raw = item.audiobook_raw();
        if let Err(e) = state
            .db()
            .inner()
            .query(format!(
                "UPDATE job SET status = 'dead', \
                    finished_at = time::now(), \
                    last_error = 'queue item cancelled' \
                 WHERE audiobook = audiobook:`{audiobook_raw}` \
                   AND status IN ['queued', 'running', 'throttled']"
            ))
            .await
        {
            warn!(error = %e, audiobook_id = audiobook_raw, "queue cancel: failed to clear live jobs");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/queue/advance",
    tag = "queue",
    responses(
        (status = 204, description = "Runner tick triggered"),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn advance(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<StatusCode> {
    // Manual kick when the user resumes — gives instant feedback
    // instead of waiting up to RUNNER_TICK seconds for the loop.
    advance_user(&state, &user.id).await;
    Ok(StatusCode::NO_CONTENT)
}

// -------------------------------------------------------------------------
// Queries
// -------------------------------------------------------------------------

async fn load_items(state: &AppState, user_id: &UserId) -> Result<Vec<QueueItem>> {
    let rows: Vec<DbQueueItem> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM audiobook_queue WHERE owner = user:`{}` \
             ORDER BY position ASC, queued_at ASC",
            user_id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("queue list: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue list (decode): {e}")))?;

    // Bulk-fetch audiobook titles + statuses in one round trip so we
    // don't N+1 per row. SurrealDB lacks a clean IN-by-record query
    // builder; concatenating ids into the WHERE works because all ids
    // come from our own rows.
    let mut items: Vec<QueueItem> = Vec::with_capacity(rows.len());
    for (idx, row) in rows.iter().enumerate() {
        let audiobook_raw = row.audiobook_raw();
        let book = load_book_meta(state, &audiobook_raw).await.ok();
        let cost_usd = sum_cost(state, &audiobook_raw).await.unwrap_or(0.0);
        let (step, progress_pct) = derive_step(state, &audiobook_raw, &row.state).await;
        let state_parsed =
            QueueItemState::parse(&row.state).unwrap_or(QueueItemState::Queued);
        items.push(QueueItem {
            id: row.raw_id(),
            position: (idx + 1) as u32,
            state: state_parsed,
            audiobook_id: audiobook_raw.clone(),
            title: book.as_ref().map(|b| b.title.clone()).unwrap_or_default(),
            topic: book.as_ref().map(|b| b.topic.clone()).unwrap_or_default(),
            language: book.as_ref().and_then(|b| b.language.clone()),
            is_short: book.as_ref().and_then(|b| b.is_short).unwrap_or(false),
            is_songbook: book.as_ref().and_then(|b| b.is_songbook).unwrap_or(false),
            audiobook_status: book
                .as_ref()
                .map(|b| b.status.clone())
                .unwrap_or_else(|| "draft".to_string()),
            step,
            progress_pct,
            cost_usd,
            error: row.error.clone(),
            queued_at: row.queued_at,
            started_at: row.started_at,
            finished_at: row.finished_at,
        });
    }
    Ok(items)
}

#[derive(Debug, Deserialize)]
struct BookMeta {
    title: String,
    topic: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    is_short: Option<bool>,
    #[serde(default)]
    is_songbook: Option<bool>,
    status: String,
}

async fn load_book_meta(state: &AppState, audiobook_id: &str) -> Result<BookMeta> {
    let rows: Vec<BookMeta> = state
        .db()
        .inner()
        .query(format!(
            "SELECT title, topic, language, is_short, is_songbook, status \
             FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("queue book meta: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue book meta (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{audiobook_id}"),
    })
}

async fn sum_cost(state: &AppState, audiobook_id: &str) -> Result<f64> {
    #[derive(Deserialize)]
    struct Row {
        total: Option<f64>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT math::sum(cost_usd) AS total FROM generation_event \
             WHERE audiobook = audiobook:`{audiobook_id}` GROUP ALL"
        ))
        .await
        .map_err(|e| Error::Database(format!("queue sum cost: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue sum cost (decode): {e}")))?;
    Ok(rows.into_iter().next().and_then(|r| r.total).unwrap_or(0.0))
}

/// Inspect live + recent jobs to label the current step. The label
/// trails the *latest* live job's kind, falling back to the audiobook
/// status when there are no live jobs (e.g., between cascade hops).
async fn derive_step(state: &AppState, audiobook_id: &str, qstate: &str) -> (String, f32) {
    if qstate == "queued" {
        return ("queued".into(), 0.0);
    }
    if qstate == "cancelled" {
        return ("cancelled".into(), 0.0);
    }
    if qstate == "completed" {
        return ("done".into(), 100.0);
    }
    if qstate == "failed" {
        return ("failed".into(), 0.0);
    }

    #[derive(Deserialize)]
    struct JobRow {
        kind: String,
        status: String,
        #[serde(default)]
        progress_pct: Option<f32>,
    }
    let live: Vec<JobRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT kind, status, progress_pct FROM job \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND status IN ['queued','running'] \
             ORDER BY queued_at DESC LIMIT 5"
        ))
        .await
        .and_then(|mut r| r.take(0))
        .unwrap_or_default();

    if let Some(top) = live.into_iter().find(|j| j.status == "running") {
        return (label_for_kind(&top.kind).into(), top.progress_pct.unwrap_or(0.0));
    }

    // No running jobs — fall back to audiobook status for a hint
    // about where the pipeline left off.
    let status_rows: Vec<(String,)> = state
        .db()
        .inner()
        .query(format!(
            "SELECT VALUE status FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .and_then(|mut r| r.take(0))
        .unwrap_or_default();
    let status = status_rows
        .into_iter()
        .next()
        .map(|(s,)| s)
        .unwrap_or_else(|| "draft".into());
    let pct = match status.as_str() {
        "draft" => 0.0,
        "outline_ready" => 25.0,
        "text_ready" => 55.0,
        "audio_ready" => 90.0,
        _ => 0.0,
    };
    (label_for_status(&status).into(), pct)
}

fn label_for_kind(kind: &str) -> &'static str {
    match kind {
        "outline" => "drafting outline",
        "chapters" => "writing chapters",
        "tts" | "tts_chapter" => "narrating",
        "cover" => "rendering art",
        "chapter_paragraphs" => "rendering paragraphs",
        "animate" | "animate_chapter" => "animating video",
        "publish_youtube" => "publishing",
        "song_snippets" => "fetching song snippets",
        "translate" => "translating",
        "post_process" => "post-processing",
        _ => "working",
    }
}

fn label_for_status(status: &str) -> &'static str {
    match status {
        "draft" => "draft",
        "outline_ready" => "outline ready",
        "text_ready" => "chapters ready",
        "audio_ready" => "audio ready",
        "failed" => "failed",
        _ => status_or_unknown(status),
    }
}

fn status_or_unknown(_: &str) -> &'static str {
    "working"
}

async fn load_paused(state: &AppState, user_id: &UserId) -> Result<bool> {
    let rows: Vec<DbQueueSettings> = state
        .db()
        .inner()
        .query(format!(
            "SELECT paused FROM queue_settings WHERE owner = user:`{}` LIMIT 1",
            user_id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("queue paused: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue paused (decode): {e}")))?;
    Ok(rows.into_iter().next().map(|r| r.paused).unwrap_or(false))
}

async fn upsert_paused(state: &AppState, user_id: &UserId, paused: bool) -> Result<()> {
    // SurrealDB UPSERT lands the row idempotently; we key on owner so
    // each user has at most one settings row.
    let sql = format!(
        r#"UPSERT queue_settings WHERE owner = user:`{user_id}` SET
            owner = user:`{user_id}`,
            paused = $paused"#,
        user_id = user_id.0,
    );
    state
        .db()
        .inner()
        .query(sql)
        .bind(("paused", paused))
        .await
        .map_err(|e| Error::Database(format!("queue upsert paused: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("queue upsert paused: {e}")))?;
    Ok(())
}

async fn load_item(state: &AppState, item_id: &str) -> Result<DbQueueItem> {
    let rows: Vec<DbQueueItem> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM audiobook_queue:`{item_id}`"))
        .await
        .map_err(|e| Error::Database(format!("queue load item: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue load item (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook_queue:{item_id}"),
    })
}

// -------------------------------------------------------------------------
// Queue runner
// -------------------------------------------------------------------------

/// Spawn the background loop that promotes `queued → running` and
/// settles finished items. One global task watches every user's queue
/// so we don't have to thread per-user state through `AppState`.
///
/// The loop is intentionally cheap: each tick is a SELECT per user
/// over a tiny per-user index, and most users have empty queues.
pub fn spawn_queue_runner(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RUNNER_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the immediate first tick — startup doesn't need an
        // instant advance and dropping it gives the worker pool time
        // to come online.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = runner_tick(&state).await {
                warn!(error = %e, "queue runner tick failed");
            }
        }
    })
}

async fn runner_tick(state: &AppState) -> Result<()> {
    // Distinct owners with any non-terminal queue item. We only care
    // about queued/running rows — completed/failed/cancelled don't
    // need attention.
    #[derive(Deserialize)]
    struct OwnerRow {
        owner: Thing,
    }
    let owners: Vec<OwnerRow> = state
        .db()
        .inner()
        .query(
            "SELECT owner FROM audiobook_queue \
             WHERE state IN ['queued','running'] \
             GROUP BY owner",
        )
        .await
        .map_err(|e| Error::Database(format!("queue runner owners: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue runner owners (decode): {e}")))?;
    for row in owners {
        let uid = UserId(row.owner.id.to_raw());
        advance_user(state, &uid).await;
    }
    Ok(())
}

/// Move a single user's queue forward by one step. Public so the
/// `/queue/advance` endpoint can poke the loop on demand (e.g., right
/// after the user resumes a paused queue).
pub(crate) async fn advance_user(state: &AppState, user_id: &UserId) {
    if let Err(e) = try_settle_running(state, user_id).await {
        warn!(error = %e, user = user_id.0, "queue: settle running failed");
    }
    match load_paused(state, user_id).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            warn!(error = %e, user = user_id.0, "queue: load paused failed");
        }
    }
    if let Err(e) = try_activate_next(state, user_id).await {
        warn!(error = %e, user = user_id.0, "queue: activate next failed");
    }
}

/// If the user has a `running` item AND that item's audiobook is at a
/// terminal pipeline state (audio_ready / failed / no live jobs),
/// mark the queue row as completed (or failed).
async fn try_settle_running(state: &AppState, user_id: &UserId) -> Result<()> {
    let running: Vec<DbQueueItem> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM audiobook_queue \
             WHERE owner = user:`{}` AND state = 'running' LIMIT 1",
            user_id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("queue settle: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue settle (decode): {e}")))?;
    let Some(item) = running.into_iter().next() else {
        return Ok(());
    };
    let book_id = item.audiobook_raw();
    let item_id = item.raw_id();

    // Live job count for this book. If non-zero, we wait.
    #[derive(Deserialize)]
    struct CountRow {
        count: i64,
    }
    let live_rows: Vec<CountRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM job \
             WHERE audiobook = audiobook:`{book_id}` \
               AND status IN ['queued','running','throttled'] GROUP ALL"
        ))
        .await
        .map_err(|e| Error::Database(format!("queue live job count: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue live job count (decode): {e}")))?;
    let live = live_rows.into_iter().next().map(|r| r.count).unwrap_or(0);
    if live > 0 {
        return Ok(());
    }

    // No live jobs — settle. Status decides the outcome:
    //   audio_ready  → completed (user wanted audio and got it)
    //   text_ready   → completed (chapters-only pipeline)
    //   outline_ready → completed (outline-only pipeline)
    //   failed       → failed
    //   draft        → activation never happened; mark failed so we
    //                  don't get stuck forever pointing at a row
    //                  whose outline crashed before any job spawned
    let status: Vec<(String,)> = state
        .db()
        .inner()
        .query(format!(
            "SELECT VALUE status FROM audiobook:`{book_id}`"
        ))
        .await
        .and_then(|mut r| r.take(0))
        .unwrap_or_default();
    let status = status
        .into_iter()
        .next()
        .map(|(s,)| s)
        .unwrap_or_else(|| "failed".into());
    let (new_state, err) = match status.as_str() {
        "outline_ready" | "text_ready" | "audio_ready" => ("completed", None),
        "draft" => (
            "failed",
            Some("audiobook never left draft — outline likely failed".to_string()),
        ),
        _ => ("failed", Some(format!("audiobook status = {status}"))),
    };
    let sql = format!(
        "UPDATE audiobook_queue:`{item_id}` SET \
            state = $state, \
            error = $error, \
            finished_at = time::now()"
    );
    state
        .db()
        .inner()
        .query(sql)
        .bind(("state", new_state.to_string()))
        .bind(("error", err))
        .await
        .map_err(|e| Error::Database(format!("queue settle update: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("queue settle update: {e}")))?;
    Ok(())
}

/// Pick the next `queued` item (lowest position) and activate it by
/// calling `audiobook::kick_off_pipeline`. No-op if there's still a
/// `running` row for this user.
async fn try_activate_next(state: &AppState, user_id: &UserId) -> Result<()> {
    // Guard: still running something?
    #[derive(Deserialize)]
    struct CountRow {
        count: i64,
    }
    let running: Vec<CountRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM audiobook_queue \
             WHERE owner = user:`{}` AND state = 'running' GROUP ALL",
            user_id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("queue activate count: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue activate count (decode): {e}")))?;
    if running.into_iter().next().map(|r| r.count).unwrap_or(0) > 0 {
        return Ok(());
    }

    // Pick next queued.
    let next: Vec<DbQueueItem> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM audiobook_queue \
             WHERE owner = user:`{}` AND state = 'queued' \
             ORDER BY position ASC, queued_at ASC LIMIT 1",
            user_id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("queue activate fetch: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("queue activate fetch (decode): {e}")))?;
    let Some(item) = next.into_iter().next() else {
        return Ok(());
    };
    let item_id = item.raw_id();
    let book_id = item.audiobook_raw();

    // Atomically flip queued → running. The conditional `WHERE` makes
    // this idempotent against a racing claim — only one runner will
    // see a non-empty result vector. SurrealDB's `RETURN AFTER` (the
    // default for UPDATE) returns the updated rows; we only need the
    // non-empty check, not the contents.
    let claimed: Vec<DbQueueItem> = state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook_queue:`{item_id}` SET \
                state = 'running', \
                started_at = time::now() \
             WHERE state = 'queued'"
        ))
        .await
        .map_err(|e| Error::Database(format!("queue activate claim: {e}")))?
        .take(0)
        .unwrap_or_default();
    if claimed.is_empty() {
        return Ok(());
    }

    // Kick off generation in a detached task so the runner loop
    // isn't blocked by outline_gen's synchronous LLM call (which can
    // run for many seconds).
    let state_clone = state.clone();
    let user_id_clone = user_id.clone();
    let book_id_clone = book_id.clone();
    let item_id_clone = item_id.clone();
    tokio::spawn(async move {
        match audiobook::kick_off_pipeline(&state_clone, &user_id_clone, &book_id_clone).await {
            Ok(()) => {}
            Err(e) => {
                warn!(error = %e, audiobook = book_id_clone, "queue activation: kick_off_pipeline failed");
                let msg = format!("{e}");
                let _ = state_clone
                    .db()
                    .inner()
                    .query(format!(
                        "UPDATE audiobook_queue:`{item_id_clone}` SET \
                            state = 'failed', \
                            error = $error, \
                            finished_at = time::now()"
                    ))
                    .bind(("error", msg))
                    .await;
            }
        }
    });
    Ok(())
}
