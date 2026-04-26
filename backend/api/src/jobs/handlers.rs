//! Concrete `JobHandler` implementations for each `JobKind` this service
//! actually executes. Each handler closes over [`AppState`] so it can reach
//! the LLM / TTS clients, config, and storage paths.

use std::time::Duration;

use async_trait::async_trait;
use listenai_core::domain::JobKind;
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use listenai_jobs::{
    handler::{JobContext, JobHandlerRegistry, JobHandlerRegistryBuilder, JobOutcome},
    repo::{EnqueueRequest, JobRow},
    JobHandler,
};
use serde::Deserialize;
use tracing::{info, warn};

use crate::generation::{audio as audio_gen, chapter as chapter_gen, translate as translate_gen};
use crate::state::AppState;

/// Build the per-process handler registry. Call once at boot.
pub fn registry(state: AppState) -> JobHandlerRegistry {
    JobHandlerRegistryBuilder::default()
        .register(JobKind::Chapters, ChaptersHandler(state.clone()))
        .register(JobKind::Tts, TtsParentHandler(state.clone()))
        .register(JobKind::TtsChapter, TtsChapterHandler(state.clone()))
        .register(JobKind::Translate, TranslateHandler(state.clone()))
        .register(JobKind::Gc, GcHandler(state))
        .build()
}

// ---------------------------------------------------------------------------
// Chapters: sequential LLM writer for the whole book.
// ---------------------------------------------------------------------------

struct ChaptersHandler(AppState);

#[async_trait]
impl JobHandler for ChaptersHandler {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome> {
        let user = job.user_id.clone().ok_or_else(|| {
            Error::Database("chapters job missing user".into())
        })?;
        let audiobook_id = job.audiobook_id.clone().ok_or_else(|| {
            Error::Database("chapters job missing audiobook".into())
        })?;

        ctx.progress(&job, "starting", 0.0).await;
        // Sequential by design — chapter N uses chapter N-1's ending for
        // continuity, so parallelising here would cost quality.
        match chapter_gen::run_all(&self.0, &UserId(user), &audiobook_id).await {
            Ok(()) => {
                ctx.progress(&job, "text_ready", 1.0).await;
                Ok(JobOutcome::Done)
            }
            Err(e) => Ok(JobOutcome::Retry(e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// TTS parent: fans out one TtsChapter child per chapter and aggregates.
// ---------------------------------------------------------------------------

struct TtsParentHandler(AppState);

#[derive(Debug, Deserialize)]
struct ChapterRefRow {
    number: i64,
}

#[async_trait]
impl JobHandler for TtsParentHandler {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome> {
        let user_id = job.user_id.clone().ok_or_else(|| {
            Error::Database("tts job missing user".into())
        })?;
        let audiobook_id = job.audiobook_id.clone().ok_or_else(|| {
            Error::Database("tts job missing audiobook".into())
        })?;

        set_audiobook_status(&self.0, &audiobook_id, "chapters_running").await?;
        ctx.progress(&job, "fan_out", 0.0).await;

        // Default the parent's language to the audiobook's primary language
        // when the caller didn't specify one (legacy / pre-multilang flows).
        let language = match &job.language {
            Some(l) => l.clone(),
            None => primary_language(&self.0, &audiobook_id).await?,
        };

        // Enumerate chapters to narrate in the target language.
        let rows: Vec<ChapterRefRow> = self
            .0
            .db()
            .inner()
            .query(format!(
                "SELECT number FROM chapter \
                 WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang \
                 ORDER BY number ASC"
            ))
            .bind(("lang", language.clone()))
            .await
            .map_err(|e| Error::Database(format!("tts fan-out load: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("tts fan-out load (decode): {e}")))?;

        if rows.is_empty() {
            set_audiobook_status(&self.0, &audiobook_id, "failed").await?;
            return Ok(JobOutcome::Fatal("no chapters to narrate".into()));
        }

        let total = rows.len();
        let parent_id = listenai_core::id::JobId(job.id.clone());

        // Enqueue every child. Cheap — just INSERTs.
        for ch in &rows {
            let req = EnqueueRequest::new(JobKind::TtsChapter)
                .with_user(UserId(user_id.clone()))
                .with_audiobook(listenai_core::id::AudiobookId(audiobook_id.clone()))
                .with_parent(parent_id.clone())
                .with_chapter(ch.number as u32)
                .with_language(language.clone())
                .with_max_attempts(3);
            if let Err(e) = ctx.repo.enqueue(req).await {
                // If fan-out partially fails, try to recover by marking this
                // parent as retryable; the already-enqueued children will
                // still run and be visible to the next attempt.
                warn!(
                    audiobook = %audiobook_id,
                    chapter = ch.number,
                    error = %e,
                    "enqueue child tts failed"
                );
                return Ok(JobOutcome::Retry(format!("child enqueue failed: {e}")));
            }
        }

        // Aggregate: poll children until all terminal.
        loop {
            let children = ctx.repo.children(&job.id).await?;
            let done = children
                .iter()
                .filter(|c| c.status.is_terminal())
                .count();
            let any_dead = children
                .iter()
                .any(|c| c.status == listenai_core::domain::JobStatus::Dead);
            let pct = (done as f32 / total as f32).clamp(0.0, 1.0);
            ctx.progress(&job, "narrating", pct).await;

            if done == total {
                let final_status = if any_dead { "failed" } else { "audio_ready" };
                set_audiobook_status(&self.0, &audiobook_id, final_status).await?;
                info!(
                    audiobook = %audiobook_id,
                    chapters = total,
                    failed = any_dead,
                    "tts parent complete"
                );
                return if any_dead {
                    Ok(JobOutcome::Fatal("one or more chapters failed".into()))
                } else {
                    Ok(JobOutcome::Done)
                };
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// TtsChapter: the real synthesis worker (up to 4 in parallel).
// ---------------------------------------------------------------------------

struct TtsChapterHandler(AppState);

#[async_trait]
impl JobHandler for TtsChapterHandler {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome> {
        let user_id = job.user_id.clone().ok_or_else(|| {
            Error::Database("tts_chapter job missing user".into())
        })?;
        let audiobook_id = job.audiobook_id.clone().ok_or_else(|| {
            Error::Database("tts_chapter job missing audiobook".into())
        })?;
        let chapter_number = job.chapter_number.ok_or_else(|| {
            Error::Database("tts_chapter job missing chapter_number".into())
        })?;
        let language = match &job.language {
            Some(l) => l.clone(),
            None => primary_language(&self.0, &audiobook_id).await?,
        };

        ctx.progress(&job, "narrating", 0.0).await;
        match audio_gen::run_one_by_number(
            &self.0,
            &UserId(user_id),
            &audiobook_id,
            chapter_number as i64,
            &language,
        )
        .await
        {
            Ok(()) => {
                ctx.progress(&job, "audio_ready", 1.0).await;
                Ok(JobOutcome::Done)
            }
            Err(e) => Ok(JobOutcome::Retry(e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Translate: LLM-based chapter translator from one language to another.
// ---------------------------------------------------------------------------

struct TranslateHandler(AppState);

#[async_trait]
impl JobHandler for TranslateHandler {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome> {
        let user = job.user_id.clone().ok_or_else(|| {
            Error::Database("translate job missing user".into())
        })?;
        let audiobook_id = job.audiobook_id.clone().ok_or_else(|| {
            Error::Database("translate job missing audiobook".into())
        })?;
        let target = job
            .language
            .clone()
            .ok_or_else(|| Error::Database("translate job missing language".into()))?;
        let source = job
            .payload
            .as_ref()
            .and_then(|p| p.get("source_language"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| "en".to_string());

        ctx.progress(&job, "translating", 0.0).await;
        match translate_gen::translate_audiobook(
            &self.0,
            &UserId(user),
            &audiobook_id,
            &source,
            &target,
        )
        .await
        {
            Ok(created) => {
                info!(
                    audiobook = %audiobook_id,
                    target = %target,
                    chapters = created,
                    "translation complete"
                );
                ctx.progress(&job, "completed", 1.0).await;
                Ok(JobOutcome::Done)
            }
            Err(e) => Ok(JobOutcome::Retry(e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// GC: nightly orphan-audio sweep.
// ---------------------------------------------------------------------------

struct GcHandler(AppState);

#[async_trait]
impl JobHandler for GcHandler {
    async fn run(&self, _ctx: &JobContext, _job: JobRow) -> Result<JobOutcome> {
        let storage = self.0.config().storage_path.clone();
        if !storage.exists() {
            return Ok(JobOutcome::Done);
        }

        // Read current audiobook ids so we know which dirs are live.
        #[derive(Deserialize)]
        struct Row {
            id: surrealdb::sql::Thing,
        }
        let rows: Vec<Row> = self
            .0
            .db()
            .inner()
            .query("SELECT id FROM audiobook")
            .await
            .map_err(|e| Error::Database(format!("gc list audiobooks: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("gc list audiobooks (decode): {e}")))?;
        let live: std::collections::HashSet<String> =
            rows.into_iter().map(|r| r.id.id.to_raw()).collect();

        let mut removed = 0usize;
        let read_dir = match std::fs::read_dir(&storage) {
            Ok(rd) => rd,
            Err(e) => {
                warn!(error = %e, path = ?storage, "gc: cannot read storage dir");
                return Ok(JobOutcome::Done);
            }
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if live.contains(name) {
                continue;
            }
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {
                    removed += 1;
                    info!(path = ?path, "gc: removed orphan audio dir");
                }
                Err(e) => warn!(error = %e, path = ?path, "gc: remove failed"),
            }
        }
        info!(removed, "gc sweep complete");
        Ok(JobOutcome::Done)
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

async fn set_audiobook_status(
    state: &AppState,
    audiobook_id: &str,
    status: &str,
) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook:`{audiobook_id}` SET status = $status"
        ))
        .bind(("status", status.to_string()))
        .await
        .map_err(|e| Error::Database(format!("set audiobook status: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set audiobook status: {e}")))?;
    Ok(())
}

async fn primary_language(state: &AppState, audiobook_id: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        language: Option<String>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT language FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("primary_language: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("primary_language (decode): {e}")))?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.language)
        .unwrap_or_else(|| "en".to_string()))
}
