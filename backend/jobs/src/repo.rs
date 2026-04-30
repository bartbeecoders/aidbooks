//! Typed SurrealQL for the `job` table.
//!
//! All "leasing" semantics live here: the pickup query flips `queued → running`
//! atomically (SurrealDB serialises writes per record, so two workers racing
//! on the same row lose one of the writes and see no rows returned). Failure
//! paths either re-queue with exponential backoff or mark the job dead.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use listenai_core::domain::{JobKind, JobStatus};
use listenai_core::id::{AudiobookId, JobId, UserId};
use listenai_core::{Error, Result};
use listenai_db::Db;
use serde::Deserialize;
use surrealdb::sql::Thing;
use tracing::{debug, warn};

/// Decoded job row — strings instead of `surrealdb::sql::Thing`, so handlers
/// never have to reach into the SurrealDB types.
#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: String,
    pub kind: JobKind,
    pub user_id: Option<String>,
    pub audiobook_id: Option<String>,
    pub parent_id: Option<String>,
    pub chapter_number: Option<u32>,
    /// BCP-47 language target for chapter/tts jobs. Lets a single audiobook
    /// have parallel narration jobs (one per language).
    pub language: Option<String>,
    pub status: JobStatus,
    pub progress_pct: f32,
    pub attempts: u32,
    pub max_attempts: u32,
    pub last_error: Option<String>,
    pub worker_id: Option<String>,
    pub queued_at: DateTime<Utc>,
    pub not_before: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct DbJob {
    id: Thing,
    kind: String,
    user: Option<Thing>,
    audiobook: Option<Thing>,
    parent: Option<Thing>,
    chapter_number: Option<i64>,
    #[serde(default)]
    language: Option<String>,
    status: String,
    progress_pct: f32,
    attempts: i64,
    max_attempts: i64,
    last_error: Option<String>,
    worker_id: Option<String>,
    queued_at: DateTime<Utc>,
    not_before: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    payload: Option<serde_json::Value>,
}

impl DbJob {
    fn decode(self) -> Result<JobRow> {
        Ok(JobRow {
            id: self.id.id.to_raw(),
            kind: JobKind::parse(&self.kind)
                .ok_or_else(|| Error::Database(format!("unknown job kind `{}`", self.kind)))?,
            user_id: self.user.map(|t| t.id.to_raw()),
            audiobook_id: self.audiobook.map(|t| t.id.to_raw()),
            parent_id: self.parent.map(|t| t.id.to_raw()),
            chapter_number: self.chapter_number.map(|n| n as u32),
            language: self.language,
            status: JobStatus::parse(&self.status)
                .ok_or_else(|| Error::Database(format!("unknown status `{}`", self.status)))?,
            progress_pct: self.progress_pct,
            attempts: self.attempts as u32,
            max_attempts: self.max_attempts as u32,
            last_error: self.last_error,
            worker_id: self.worker_id,
            queued_at: self.queued_at,
            not_before: self.not_before,
            started_at: self.started_at,
            finished_at: self.finished_at,
            updated_at: self.updated_at,
            payload: self.payload,
        })
    }
}

/// Caller-facing enqueue request. `id` is optional — if set, the row is
/// upserted (idempotent enqueue). Phase-5 handlers use this when reusing a
/// job id between a retry and an idempotency-key replay.
#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    pub id: Option<JobId>,
    pub kind: JobKind,
    pub user: Option<UserId>,
    pub audiobook: Option<AudiobookId>,
    pub parent: Option<JobId>,
    pub chapter_number: Option<u32>,
    pub language: Option<String>,
    pub max_attempts: u32,
    pub payload: Option<serde_json::Value>,
}

impl EnqueueRequest {
    pub fn new(kind: JobKind) -> Self {
        Self {
            id: None,
            kind,
            user: None,
            audiobook: None,
            parent: None,
            chapter_number: None,
            language: None,
            max_attempts: 3,
            payload: None,
        }
    }
    pub fn with_user(mut self, u: UserId) -> Self {
        self.user = Some(u);
        self
    }
    pub fn with_audiobook(mut self, a: AudiobookId) -> Self {
        self.audiobook = Some(a);
        self
    }
    pub fn with_parent(mut self, p: JobId) -> Self {
        self.parent = Some(p);
        self
    }
    pub fn with_chapter(mut self, n: u32) -> Self {
        self.chapter_number = Some(n);
        self
    }
    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = Some(lang.into());
        self
    }
    pub fn with_max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = n;
        self
    }
    pub fn with_payload(mut self, p: serde_json::Value) -> Self {
        self.payload = Some(p);
        self
    }
}

#[derive(Clone)]
pub struct JobRepo {
    db: Db,
}

impl JobRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Insert a new job in `queued` state. Returns the assigned id.
    pub async fn enqueue(&self, req: EnqueueRequest) -> Result<JobId> {
        let id = req.id.unwrap_or_default();

        let user_set = req
            .user
            .as_ref()
            .map(|u| format!(", user: user:`{}`", u.0))
            .unwrap_or_default();
        let book_set = req
            .audiobook
            .as_ref()
            .map(|a| format!(", audiobook: audiobook:`{}`", a.0))
            .unwrap_or_default();
        let parent_set = req
            .parent
            .as_ref()
            .map(|p| format!(", parent: job:`{}`", p.0))
            .unwrap_or_default();
        let chap_set = req
            .chapter_number
            .map(|n| format!(", chapter_number: {n}"))
            .unwrap_or_default();
        let lang_set = if req.language.is_some() {
            ", language: $language"
        } else {
            ""
        };

        let sql = format!(
            r#"CREATE job:`{jid}` CONTENT {{
                kind: $kind,
                status: "queued",
                progress_pct: 0.0,
                attempts: 0,
                max_attempts: $max_attempts,
                payload: $payload
                {user_set}
                {book_set}
                {parent_set}
                {chap_set}
                {lang_set}
            }}"#,
            jid = id.0,
        );

        self.db
            .inner()
            .query(sql)
            .bind(("kind", req.kind.as_str().to_string()))
            .bind(("max_attempts", req.max_attempts as i64))
            .bind(("payload", req.payload))
            .bind(("language", req.language.clone()))
            .await
            .map_err(|e| Error::Database(format!("enqueue job: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("enqueue job: {e}")))?;

        debug!(job_id = %id.0, kind = req.kind.as_str(), "job enqueued");
        Ok(id)
    }

    /// Atomic pickup. Returns the freshly-leased job (with `attempts+=1`,
    /// `status=running`, `started_at=now`) or `None` if nothing is eligible.
    ///
    /// We select `queued` jobs whose `kind` is in `kinds` and whose
    /// `not_before` is due. Sorted FIFO by `queued_at`. The first matching
    /// row is leased to this worker.
    pub async fn pick_next(&self, worker_id: &str, kinds: &[JobKind]) -> Result<Option<JobRow>> {
        if kinds.is_empty() {
            return Ok(None);
        }
        let kind_strs: Vec<String> = kinds.iter().map(|k| k.as_str().to_string()).collect();

        // SurrealDB does not let an `UPDATE` have an `ORDER BY`, so we do the
        // select-then-update dance inside a transaction. Two workers racing
        // on the same row will both write, but the per-record write order
        // ensures attempts increments serially; we then post-check the row
        // we got back (`status=running` + `worker_id` match) to claim it.
        // SurrealDB (v2.6) insists that the ORDER BY field be part of the
        // projection, so we project `id, queued_at` and then index into the
        // row to recover `id`.
        let sql = r#"
            BEGIN TRANSACTION;
            LET $candidates = (SELECT id, queued_at FROM job
                WHERE status = "queued"
                  AND kind INSIDE $kinds
                  AND not_before <= time::now()
                ORDER BY queued_at ASC LIMIT 1);
            LET $picked = IF array::len($candidates) = 0 THEN NONE ELSE $candidates[0].id END;
            RETURN IF $picked IS NONE THEN [] ELSE (
                UPDATE $picked SET
                    status = "running",
                    worker_id = $worker,
                    started_at = time::now(),
                    updated_at = time::now(),
                    attempts = attempts + 1
                WHERE status = "queued"
                RETURN AFTER
            ) END;
            COMMIT TRANSACTION;
        "#;

        let mut res = self
            .db
            .inner()
            .query(sql)
            .bind(("kinds", kind_strs))
            .bind(("worker", worker_id.to_string()))
            .await
            .map_err(|e| Error::Database(format!("pick job: {e}")))?;

        let rows: Vec<DbJob> = res
            .take(0)
            .map_err(|e| Error::Database(format!("pick job (decode): {e}")))?;

        match rows.into_iter().next() {
            Some(j) => {
                let leased = j.decode()?;
                // Another worker may have raced and leased it; only claim it
                // if the row-back confirms our worker id.
                if leased.worker_id.as_deref() == Some(worker_id) {
                    Ok(Some(leased))
                } else {
                    warn!(
                        job_id = %leased.id,
                        expected = %worker_id,
                        actual = ?leased.worker_id,
                        "lost pickup race"
                    );
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Publish running-progress; does NOT flip status.
    pub async fn set_progress(&self, job_id: &str, pct: f32) -> Result<()> {
        self.db
            .inner()
            .query(format!(
                "UPDATE job:`{job_id}` SET progress_pct = $pct, updated_at = time::now()"
            ))
            .bind(("pct", pct.clamp(0.0, 1.0) as f64))
            .await
            .map_err(|e| Error::Database(format!("set_progress: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("set_progress: {e}")))?;
        Ok(())
    }

    pub async fn mark_completed(&self, job_id: &str) -> Result<()> {
        // Gated on `status = "running"` so an admin who cancels a mid-flight
        // job doesn't get the cancel overwritten when the worker finishes.
        // The worker's terminal write becomes a no-op in that case.
        self.db
            .inner()
            .query(format!(
                r#"UPDATE job:`{job_id}` SET
                    status = "completed",
                    progress_pct = 1.0,
                    finished_at = time::now(),
                    updated_at = time::now(),
                    last_error = NONE,
                    worker_id = NONE
                  WHERE status = "running"
                "#
            ))
            .await
            .map_err(|e| Error::Database(format!("mark_completed: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("mark_completed: {e}")))?;
        Ok(())
    }

    /// Record a failure. If `attempts < max_attempts` the job is requeued
    /// with exponential backoff (`base_backoff * 2^attempts`), otherwise
    /// it lands in `dead`. Returns whether the job is now terminal.
    pub async fn mark_failed(
        &self,
        row: &JobRow,
        error: &str,
        base_backoff: ChronoDuration,
    ) -> Result<bool> {
        let terminal = row.attempts >= row.max_attempts;
        if terminal {
            // Same gating reason as `mark_completed`: an admin's cancel-flip
            // wins over a worker's terminal write.
            self.db
                .inner()
                .query(format!(
                    r#"UPDATE job:`{jid}` SET
                        status = "dead",
                        finished_at = time::now(),
                        updated_at = time::now(),
                        last_error = $err,
                        worker_id = NONE
                      WHERE status = "running"
                    "#,
                    jid = row.id
                ))
                .bind(("err", error.to_string()))
                .await
                .map_err(|e| Error::Database(format!("mark_failed(dead): {e}")))?
                .check()
                .map_err(|e| Error::Database(format!("mark_failed(dead): {e}")))?;
            return Ok(true);
        }

        // 2^attempts seconds * base, capped at 10 minutes to avoid
        // pathological waits.
        let exp_secs =
            (base_backoff.num_seconds().max(1) * 2i64.pow(row.attempts.min(8))).min(600);
        let sql = format!(
            r#"UPDATE job:`{jid}` SET
                status = "queued",
                worker_id = NONE,
                last_error = $err,
                not_before = time::now() + {exp_secs}s,
                updated_at = time::now()
              WHERE status = "running"
            "#,
            jid = row.id
        );
        self.db
            .inner()
            .query(sql)
            .bind(("err", error.to_string()))
            .await
            .map_err(|e| Error::Database(format!("mark_failed(requeue): {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("mark_failed(requeue): {e}")))?;
        Ok(false)
    }

    /// On boot, any row stuck in `running` belongs to a previous process
    /// generation that died mid-flight. Flip them back to `queued` so a
    /// fresh worker can retry. Cheap — runs once per startup.
    pub async fn recover_stalled(&self) -> Result<u64> {
        let mut res = self
            .db
            .inner()
            .query(
                r#"UPDATE job SET
                    status = "queued",
                    worker_id = NONE,
                    not_before = time::now(),
                    updated_at = time::now(),
                    last_error = "recovered: worker crashed while running"
                  WHERE status = "running"
                  RETURN id"#,
            )
            .await
            .map_err(|e| Error::Database(format!("recover_stalled: {e}")))?;
        let rows: Vec<serde_json::Value> = res
            .take(0)
            .map_err(|e| Error::Database(format!("recover_stalled (decode): {e}")))?;
        Ok(rows.len() as u64)
    }

    pub async fn children(&self, parent_id: &str) -> Result<Vec<JobRow>> {
        let rows: Vec<DbJob> = self
            .db
            .inner()
            .query(format!(
                "SELECT * FROM job WHERE parent = job:`{parent_id}` ORDER BY chapter_number ASC"
            ))
            .await
            .map_err(|e| Error::Database(format!("children: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("children (decode): {e}")))?;
        rows.into_iter().map(DbJob::decode).collect()
    }

    pub async fn by_id(&self, id: &str) -> Result<Option<JobRow>> {
        let rows: Vec<DbJob> = self
            .db
            .inner()
            .query(format!("SELECT * FROM job:`{id}`"))
            .await
            .map_err(|e| Error::Database(format!("job by id: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("job by id (decode): {e}")))?;
        rows.into_iter().next().map(DbJob::decode).transpose()
    }

    pub async fn list_for_audiobook(&self, audiobook_id: &str) -> Result<Vec<JobRow>> {
        let rows: Vec<DbJob> = self
            .db
            .inner()
            .query(format!(
                "SELECT * FROM job WHERE audiobook = audiobook:`{audiobook_id}` ORDER BY queued_at ASC"
            ))
            .await
            .map_err(|e| Error::Database(format!("list_for_audiobook: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("list_for_audiobook (decode): {e}")))?;
        rows.into_iter().map(DbJob::decode).collect()
    }
}
