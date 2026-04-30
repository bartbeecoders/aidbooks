//! Worker runtime. One pool of workers per [`JobKind`] with a configurable
//! concurrency cap. Each worker loops:
//!   1. `pick_next` — atomic pickup or idle-sleep.
//!   2. Dispatch to the registered handler.
//!   3. Map outcome → `mark_completed` / requeue / dead-letter.
//!
//! On shutdown the runtime cancels all idle waits, lets in-flight jobs
//! finish, then joins the worker tasks.

use std::sync::Arc;
use std::time::Duration;

use chrono::Duration as ChronoDuration;
use listenai_core::domain::JobKind;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::handler::{JobContext, JobHandlerRegistry, JobOutcome};
use crate::hub::ProgressEvent;

/// How long a worker sleeps when it sees an empty queue. Short enough that
/// a new job is picked up quickly; long enough not to spin the CPU.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Base retry backoff. Real backoff is `BASE * 2^attempts`, capped inside
/// the repo at 10 minutes.
const BASE_BACKOFF: ChronoDuration = ChronoDuration::seconds(2);

/// Fixed pool sizes per kind. Chapter + TTS_chapter dominate wall-clock
/// time, so they get the most parallelism; parent jobs (tts, chapters)
/// mostly coordinate and don't need many.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub pools: Vec<(JobKind, usize)>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            pools: vec![
                (JobKind::Outline, 2),
                (JobKind::Chapters, 2),
                (JobKind::Tts, 2),
                (JobKind::TtsChapter, 4),
                (JobKind::PostProcess, 1),
                (JobKind::Cover, 1),
                (JobKind::Gc, 1),
                (JobKind::Translate, 2),
                // YouTube publish jobs are dominated by network upload time;
                // running them in parallel mostly fights for the per-project
                // quota. One worker is plenty.
                (JobKind::PublishYoutube, 1),
                // Cheap-ish: one LLM call + a few CRUD writes. Bumping
                // beyond 2 doesn't pay because each chapter's children
                // are themselves Cover jobs with their own concurrency.
                (JobKind::ChapterParagraphs, 2),
            ],
        }
    }
}

/// Handle returned by [`spawn`]. `shutdown` signals every worker to stop at
/// its next idle tick and awaits a clean join.
pub struct WorkerHandle {
    shutdown_tx: broadcast::Sender<()>,
    workers: Vec<JoinHandle<()>>,
}

impl WorkerHandle {
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        for w in self.workers {
            // A panicking worker shouldn't stop us from joining the rest;
            // log and move on.
            if let Err(e) = w.await {
                warn!(error = %e, "worker join failed");
            }
        }
        info!("worker pool stopped");
    }
}

/// Start the worker pool. Call once at boot. The returned handle owns
/// graceful shutdown; `drop`ing it detaches without shutting down cleanly.
pub async fn spawn(
    ctx: JobContext,
    registry: JobHandlerRegistry,
    config: WorkerConfig,
) -> WorkerHandle {
    // Resume anything stuck in `running` from the previous process.
    match ctx.repo.recover_stalled().await {
        Ok(0) => debug!("no stalled jobs to recover"),
        Ok(n) => info!(count = n, "recovered stalled jobs from prior run"),
        Err(e) => warn!(error = %e, "recover_stalled failed; continuing"),
    }

    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let mut workers = Vec::new();
    let registry = Arc::new(registry);

    for (kind, size) in config.pools {
        if registry.get(kind).is_none() {
            debug!(?kind, "no handler registered; skipping pool");
            continue;
        }
        for slot in 0..size {
            let worker_id = format!("{}-{}-{}", kind.as_str(), slot, Uuid::new_v4().simple());
            let ctx = ctx.clone();
            let registry = registry.clone();
            let mut rx = shutdown_tx.subscribe();
            let handle = tokio::spawn(async move {
                run_worker(worker_id, kind, ctx, registry, &mut rx).await;
            });
            workers.push(handle);
        }
    }

    info!(worker_count = workers.len(), "worker pool started");
    WorkerHandle {
        shutdown_tx,
        workers,
    }
}

async fn run_worker(
    worker_id: String,
    kind: JobKind,
    ctx: JobContext,
    registry: Arc<JobHandlerRegistry>,
    shutdown: &mut broadcast::Receiver<()>,
) {
    // A worker is pinned to a single kind, so the pickup query never steals
    // jobs from a neighbouring pool. Reduces starvation risk between kinds.
    let kinds = [kind];

    loop {
        if shutdown.try_recv().is_ok() {
            break;
        }

        let picked = match ctx.repo.pick_next(&worker_id, &kinds).await {
            Ok(p) => p,
            Err(e) => {
                let msg = e.to_string();
                // SurrealDB surfaces MVCC conflicts as "read or write conflict"
                // — benign under concurrency, we just retry on the next tick.
                if msg.contains("read or write conflict") || msg.contains("can be retried") {
                    debug!(worker = %worker_id, "pick_next: transaction conflict, retrying");
                } else {
                    error!(worker = %worker_id, error = %msg, "pick_next failed");
                }
                sleep_or_shutdown(POLL_INTERVAL * 4, shutdown).await;
                continue;
            }
        };

        let Some(job) = picked else {
            sleep_or_shutdown(POLL_INTERVAL, shutdown).await;
            continue;
        };

        debug!(worker = %worker_id, job_id = %job.id, kind = job.kind.as_str(), "job picked up");

        let handler = match registry.get(job.kind) {
            Some(h) => h,
            None => {
                // Shouldn't happen — kind was registered at pool spawn — but
                // be defensive in case of racy hot-reloads.
                let _ = ctx
                    .repo
                    .mark_failed(&job, "no handler registered", BASE_BACKOFF)
                    .await;
                continue;
            }
        };

        if let Some(book) = job.audiobook_id.as_deref() {
            ctx.hub
                .publish(book, ProgressEvent::progress(&job, "started", 0.0))
                .await;
        }

        match handler.run(&ctx, job.clone()).await {
            Ok(JobOutcome::Done) => {
                if let Err(e) = ctx.repo.mark_completed(&job.id).await {
                    error!(job_id = %job.id, error = %e, "mark_completed failed");
                }
                if let Some(book) = job.audiobook_id.as_deref() {
                    ctx.hub.publish(book, ProgressEvent::completed(&job)).await;
                }
            }
            Ok(JobOutcome::Fatal(msg)) => {
                // Force dead by consuming all remaining attempts.
                let mut dead_row = job.clone();
                dead_row.attempts = job.max_attempts;
                let _ = ctx.repo.mark_failed(&dead_row, &msg, BASE_BACKOFF).await;
                if let Some(book) = job.audiobook_id.as_deref() {
                    ctx.hub
                        .publish(book, ProgressEvent::failed(&job, &msg))
                        .await;
                }
            }
            Ok(JobOutcome::Retry(msg)) => {
                handle_retry(&ctx, &job, &msg).await;
            }
            Err(e) => {
                handle_retry(&ctx, &job, &e.to_string()).await;
            }
        }
    }
    debug!(worker = %worker_id, "worker stopped");
}

async fn handle_retry(ctx: &JobContext, job: &crate::repo::JobRow, msg: &str) {
    match ctx.repo.mark_failed(job, msg, BASE_BACKOFF).await {
        Ok(true) => {
            warn!(job_id = %job.id, kind = job.kind.as_str(), error = msg, "job dead-lettered");
            if let Some(book) = job.audiobook_id.as_deref() {
                ctx.hub
                    .publish(book, ProgressEvent::failed(job, msg))
                    .await;
            }
        }
        Ok(false) => {
            warn!(job_id = %job.id, attempts = job.attempts, error = msg, "job requeued for retry");
        }
        Err(e) => error!(job_id = %job.id, error = %e, "mark_failed failed"),
    }
}

async fn sleep_or_shutdown(d: Duration, shutdown: &mut broadcast::Receiver<()>) {
    tokio::select! {
        _ = tokio::time::sleep(d) => {}
        _ = shutdown.recv() => {}
    }
}
