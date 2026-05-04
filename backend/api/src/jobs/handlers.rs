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
    hub::{JobSnapshot, ProgressEvent},
    repo::{EnqueueRequest, JobRow},
    JobHandler,
};
use serde::Deserialize;
use tracing::{info, warn};

use crate::generation::{audio as audio_gen, chapter as chapter_gen, translate as translate_gen};
use crate::jobs::publishers::animate::{AnimateChapterHandler, AnimateParentHandler};
use crate::jobs::publishers::youtube::PublishYoutubeHandler;
use crate::state::AppState;
use listenai_core::id::AudiobookId;

/// Build the per-process handler registry. Call once at boot.
pub fn registry(state: AppState) -> JobHandlerRegistry {
    JobHandlerRegistryBuilder::default()
        .register(JobKind::Chapters, ChaptersHandler(state.clone()))
        .register(JobKind::Cover, CoverHandler(state.clone()))
        .register(
            JobKind::ChapterParagraphs,
            ChapterParagraphsHandler(state.clone()),
        )
        .register(JobKind::Tts, TtsParentHandler(state.clone()))
        .register(JobKind::TtsChapter, TtsChapterHandler(state.clone()))
        .register(JobKind::Translate, TranslateHandler(state.clone()))
        .register(JobKind::PublishYoutube, PublishYoutubeHandler(state.clone()))
        .register(JobKind::Animate, AnimateParentHandler(state.clone()))
        .register(
            JobKind::AnimateChapter,
            AnimateChapterHandler::new(state.clone()),
        )
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
        match chapter_gen::run_all(&self.0, &UserId(user.clone()), &audiobook_id).await {
            Ok(()) => {
                ctx.progress(&job, "text_ready", 1.0).await;
                // Auto-pipeline: enqueue narration if the audiobook was
                // created with `auto_pipeline.audio = true`. Failures
                // here just log — the user can still hit Narrate in the
                // UI.
                let pipeline = load_auto_pipeline(&self.0, &audiobook_id).await;
                if let Some(p) = &pipeline {
                    info!(
                        audiobook_id,
                        chapters = p.chapters,
                        cover = p.cover,
                        audio = p.audio,
                        publish = p.publish.is_some(),
                        "auto-pipeline: chapters done, scheduling next steps"
                    );
                    if p.cover {
                        // Fan out one chapter-art job per chapter.
                        // Failures inside each job stay scoped — the
                        // others still run, and the user can regenerate
                        // missing tiles from the UI.
                        if let Err(e) =
                            enqueue_chapter_art_jobs(&self.0, ctx, &user, &audiobook_id).await
                        {
                            warn!(
                                error = %e,
                                audiobook_id,
                                "auto-pipeline: enqueue chapter-art jobs failed"
                            );
                        }
                        publish_job_snapshot(ctx, &audiobook_id).await;
                    }
                    if p.audio {
                        let req = EnqueueRequest::new(JobKind::Tts)
                            .with_user(UserId(user))
                            .with_audiobook(AudiobookId(audiobook_id.clone()))
                            .with_max_attempts(3);
                        match ctx.repo.enqueue(req).await {
                            Ok(id) => info!(
                                audiobook_id,
                                tts_job_id = %id.0,
                                "auto-pipeline: tts enqueued"
                            ),
                            Err(e) => warn!(
                                error = %e,
                                audiobook_id,
                                "auto-pipeline: enqueue tts after chapters failed"
                            ),
                        }
                        publish_job_snapshot(ctx, &audiobook_id).await;
                    }
                } else {
                    tracing::debug!(audiobook_id, "auto-pipeline: no pipeline config — stopping after chapters");
                }
                Ok(JobOutcome::Done)
            }
            Err(e) => Ok(JobOutcome::Retry(e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Cover art: single-shot LLM call → persist image bytes.
// ---------------------------------------------------------------------------

struct CoverHandler(AppState);

/// Audiobook fields the image-gen calls need, shared by both the main
/// cover and per-chapter branches.
#[derive(Debug, Deserialize)]
struct CoverBookRow {
    title: String,
    topic: String,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    art_style: Option<String>,
    #[serde(default)]
    cover_llm_id: Option<String>,
    #[serde(default)]
    is_short: Option<bool>,
}

#[async_trait]
impl JobHandler for CoverHandler {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome> {
        let audiobook_id = job
            .audiobook_id
            .clone()
            .ok_or_else(|| Error::Database("cover job missing audiobook".into()))?;
        let user_id = job
            .user_id
            .clone()
            .ok_or_else(|| Error::Database("cover job missing user".into()))?;
        let user = UserId(user_id);

        ctx.progress(&job, "loading", 0.0).await;

        let rows: Vec<CoverBookRow> = self
            .0
            .db()
            .inner()
            .query(format!(
                "SELECT title, topic, genre, art_style, cover_llm_id, is_short \
                 FROM audiobook:`{audiobook_id}`"
            ))
            .await
            .map_err(|e| Error::Database(format!("cover job load: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("cover job load (decode): {e}")))?;
        let book = match rows.into_iter().next() {
            Some(b) => b,
            None => return Ok(JobOutcome::Fatal(format!("audiobook {audiobook_id} not found"))),
        };

        // Branch on `chapter_number` + `payload.paragraph_index`. Same
        // Cover job kind powers (a) the main cover, (b) the per-chapter
        // cover tile, and (c) a single paragraph illustration tile.
        // The paragraph branch is fanned out by the
        // `ChapterParagraphs` orchestrator (extract LLM pass + per-tile
        // child jobs).
        if let Some(ch_n) = job.chapter_number {
            #[derive(Deserialize)]
            struct ParagraphPayload {
                #[serde(default)]
                paragraph_index: Option<u32>,
                #[serde(default)]
                ordinal: Option<u32>,
                #[serde(default)]
                total_ordinals: Option<u32>,
            }
            let para = job
                .payload
                .as_ref()
                .and_then(|v| serde_json::from_value::<ParagraphPayload>(v.clone()).ok())
                .filter(|p| p.paragraph_index.is_some());
            if let Some(p) = para {
                let idx = p.paragraph_index.unwrap_or(0);
                let ordinal = p.ordinal.unwrap_or(1);
                let total = p.total_ordinals.unwrap_or(ordinal);
                return run_paragraph_image(
                    &self.0,
                    ctx,
                    &job,
                    &user,
                    &audiobook_id,
                    ch_n,
                    idx,
                    ordinal,
                    total,
                    &book,
                )
                .await;
            }
            return run_chapter_art(&self.0, ctx, &job, &user, &audiobook_id, ch_n, &book).await;
        }

        ctx.progress(&job, "generating", 0.2).await;
        let bytes = match crate::generation::cover::generate(
            &self.0,
            &user,
            Some(&audiobook_id),
            &book.topic,
            book.genre.as_deref(),
            book.art_style.as_deref(),
            book.cover_llm_id.as_deref(),
            book.is_short.unwrap_or(false),
        )
        .await
        {
            Ok(b) => b,
            Err(e) => return Ok(JobOutcome::Retry(e.to_string())),
        };

        ctx.progress(&job, "writing", 0.9).await;
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let b64 = B64.encode(&bytes);
        if let Err(e) =
            crate::handlers::audiobook::persist_cover(&self.0, &audiobook_id, &b64).await
        {
            return Ok(JobOutcome::Retry(e.to_string()));
        }

        ctx.progress(&job, "ready", 1.0).await;
        Ok(JobOutcome::Done)
    }
}

/// Per-chapter art branch of the cover handler.
#[allow(clippy::too_many_arguments)]
async fn run_chapter_art(
    state: &AppState,
    ctx: &JobContext,
    job: &JobRow,
    user: &UserId,
    audiobook_id: &str,
    chapter_number: u32,
    book: &CoverBookRow,
) -> Result<JobOutcome> {
    #[derive(Deserialize)]
    struct ChapterRow {
        id: surrealdb::sql::Thing,
        title: String,
        #[serde(default)]
        synopsis: Option<String>,
        #[serde(default)]
        body_md: Option<String>,
    }
    // Chapter art is anchored to the *primary* language — translations
    // share the same image.
    let primary_lang = primary_language(state, audiobook_id).await?;
    let rows: Vec<ChapterRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, title, synopsis, body_md FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", chapter_number as i64))
        .bind(("lang", primary_lang))
        .await
        .map_err(|e| Error::Database(format!("chapter art load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("chapter art load (decode): {e}")))?;
    let ch = match rows.into_iter().next() {
        Some(c) => c,
        None => {
            return Ok(JobOutcome::Fatal(format!(
                "chapter {chapter_number} not found for audiobook {audiobook_id}"
            )))
        }
    };

    ctx.progress(job, "generating", 0.2).await;
    let bytes = match crate::generation::cover::generate_chapter_art(
        state,
        user,
        audiobook_id,
        &book.title,
        &book.topic,
        book.genre.as_deref(),
        book.art_style.as_deref(),
        book.cover_llm_id.as_deref(),
        chapter_number,
        &ch.title,
        ch.synopsis.as_deref(),
        ch.body_md.as_deref(),
        book.is_short.unwrap_or(false),
    )
    .await
    {
        Ok(b) => b,
        Err(e) => return Ok(JobOutcome::Retry(e.to_string())),
    };

    ctx.progress(job, "writing", 0.9).await;
    if let Err(e) = crate::handlers::audiobook::persist_chapter_art(
        state,
        audiobook_id,
        &ch.id.id.to_raw(),
        chapter_number,
        &bytes,
    )
    .await
    {
        return Ok(JobOutcome::Retry(e.to_string()));
    }
    ctx.progress(job, "ready", 1.0).await;
    Ok(JobOutcome::Done)
}

/// Single paragraph illustration tile. Loads the chapter, reads the
/// paragraph's persisted scene description (set earlier by the
/// `ChapterParagraphs` orchestrator), generates the image, and writes
/// it into `chapter.paragraphs[idx].image_paths[ordinal-1]`.
#[allow(clippy::too_many_arguments)]
async fn run_paragraph_image(
    state: &AppState,
    ctx: &JobContext,
    job: &JobRow,
    user: &UserId,
    audiobook_id: &str,
    chapter_number: u32,
    paragraph_index: u32,
    ordinal: u32,
    total_ordinals: u32,
    book: &CoverBookRow,
) -> Result<JobOutcome> {
    if ordinal == 0 || total_ordinals == 0 || ordinal > total_ordinals {
        return Ok(JobOutcome::Fatal(format!(
            "invalid ordinal {ordinal}/{total_ordinals}"
        )));
    }
    #[derive(Deserialize)]
    struct ChapterRow {
        id: surrealdb::sql::Thing,
        title: String,
        #[serde(default)]
        paragraphs: Option<Vec<serde_json::Value>>,
    }
    let primary_lang = primary_language(state, audiobook_id).await?;
    let rows: Vec<ChapterRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, title, paragraphs FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", chapter_number as i64))
        .bind(("lang", primary_lang))
        .await
        .map_err(|e| Error::Database(format!("paragraph image load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("paragraph image load (decode): {e}")))?;
    let ch = match rows.into_iter().next() {
        Some(c) => c,
        None => {
            return Ok(JobOutcome::Fatal(format!(
                "chapter {chapter_number} not found for audiobook {audiobook_id}"
            )))
        }
    };

    let paragraph = ch
        .paragraphs
        .as_ref()
        .and_then(|ps| {
            ps.iter().find(|p| {
                p.get("index")
                    .and_then(serde_json::Value::as_i64)
                    .map(|i| i == paragraph_index as i64)
                    .unwrap_or(false)
            })
        });
    let para_obj = match paragraph {
        Some(p) => p,
        None => {
            return Ok(JobOutcome::Fatal(format!(
                "paragraph {paragraph_index} missing on chapter {chapter_number}"
            )))
        }
    };
    let scene = para_obj
        .get("scene_description")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let scene = match scene {
        Some(s) => s.to_string(),
        None => {
            // Non-visual paragraph slipped through — orchestrator should
            // never enqueue these, so we fail fast rather than burn an
            // image-gen call on nothing.
            return Ok(JobOutcome::Fatal(format!(
                "paragraph {paragraph_index} has no scene_description"
            )));
        }
    };
    let text = para_obj
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    ctx.progress(job, "generating", 0.2).await;
    let bytes = match crate::generation::cover::generate_paragraph_image(
        state,
        user,
        audiobook_id,
        &book.title,
        &book.topic,
        book.genre.as_deref(),
        book.art_style.as_deref(),
        book.cover_llm_id.as_deref(),
        &ch.title,
        text,
        &scene,
        ordinal,
        total_ordinals,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => return Ok(JobOutcome::Retry(e.to_string())),
    };

    ctx.progress(job, "writing", 0.9).await;
    if let Err(e) = crate::handlers::audiobook::persist_paragraph_image(
        state,
        audiobook_id,
        &ch.id.id.to_raw(),
        chapter_number,
        paragraph_index,
        ordinal,
        &bytes,
    )
    .await
    {
        return Ok(JobOutcome::Retry(e.to_string()));
    }
    ctx.progress(job, "ready", 1.0).await;
    Ok(JobOutcome::Done)
}

// ---------------------------------------------------------------------------
// Per-chapter orchestrator: split + extract scenes + fan out tile jobs.
// ---------------------------------------------------------------------------

struct ChapterParagraphsHandler(AppState);

#[async_trait]
impl JobHandler for ChapterParagraphsHandler {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome> {
        let user_raw = job
            .user_id
            .clone()
            .ok_or_else(|| Error::Database("chapter_paragraphs missing user".into()))?;
        let audiobook_id = job
            .audiobook_id
            .clone()
            .ok_or_else(|| Error::Database("chapter_paragraphs missing audiobook".into()))?;
        let chapter_number = job.chapter_number.ok_or_else(|| {
            Error::Database("chapter_paragraphs missing chapter_number".into())
        })?;

        // The orchestrator's payload carries the per-book
        // `images_per_paragraph` knob — captured at enqueue time so
        // changes mid-flight don't half-apply.
        #[derive(Deserialize)]
        struct Payload {
            #[serde(default)]
            images_per_paragraph: Option<u32>,
        }
        let per_paragraph = job
            .payload
            .as_ref()
            .and_then(|v| serde_json::from_value::<Payload>(v.clone()).ok())
            .and_then(|p| p.images_per_paragraph)
            .unwrap_or(0)
            .clamp(0, 3);
        if per_paragraph == 0 {
            return Ok(JobOutcome::Done);
        }

        ctx.progress(&job, "loading", 0.0).await;

        // Load audiobook context for the prompt builder.
        #[derive(Deserialize)]
        struct BookRow {
            title: String,
            topic: String,
            #[serde(default)]
            genre: Option<String>,
            #[serde(default)]
            stem_detected: Option<bool>,
            #[serde(default)]
            stem_override: Option<bool>,
        }
        let mut book_resp = match self
            .0
            .db()
            .inner()
            .query(format!(
                "SELECT title, topic, genre, stem_detected, stem_override \
                 FROM audiobook:`{audiobook_id}`"
            ))
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(JobOutcome::Retry(format!("load audiobook: {e}"))),
        };
        let book: BookRow = match book_resp.take::<Vec<BookRow>>(0) {
            Ok(mut rows) => match rows.pop() {
                Some(b) => b,
                None => {
                    return Ok(JobOutcome::Fatal(format!(
                        "audiobook {audiobook_id} not found"
                    )))
                }
            },
            Err(e) => {
                return Ok(JobOutcome::Retry(format!("decode audiobook: {e}")))
            }
        };

        // Load chapter (primary language only — translations share the
        // primary's paragraphs and image set).
        let primary_lang = primary_language(&self.0, &audiobook_id).await?;
        #[derive(Deserialize)]
        struct ChapterRow {
            id: surrealdb::sql::Thing,
            title: String,
            #[serde(default)]
            body_md: Option<String>,
        }
        let mut ch_resp = match self
            .0
            .db()
            .inner()
            .query(format!(
                "SELECT id, title, body_md FROM chapter \
                 WHERE audiobook = audiobook:`{audiobook_id}` \
                   AND number = $n AND language = $lang LIMIT 1"
            ))
            .bind(("n", chapter_number as i64))
            .bind(("lang", primary_lang))
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(JobOutcome::Retry(format!("load chapter: {e}"))),
        };
        let chapter: ChapterRow = match ch_resp.take::<Vec<ChapterRow>>(0) {
            Ok(mut rows) => match rows.pop() {
                Some(c) => c,
                None => {
                    return Ok(JobOutcome::Fatal(format!(
                        "chapter {chapter_number} not found for audiobook {audiobook_id}"
                    )))
                }
            },
            Err(e) => {
                return Ok(JobOutcome::Retry(format!("decode chapter: {e}")))
            }
        };
        let body = chapter.body_md.as_deref().unwrap_or("");
        if body.trim().is_empty() {
            return Ok(JobOutcome::Fatal(format!(
                "chapter {chapter_number} has no body — extract pass needs prose"
            )));
        }

        // Split + extract scenes via the LLM.
        ctx.progress(&job, "splitting", 0.1).await;
        let paragraphs = crate::generation::paragraphs::split(body);
        if paragraphs.is_empty() {
            info!(
                audiobook = %audiobook_id,
                chapter = chapter_number,
                "chapter_paragraphs: no paragraphs cleared length filter"
            );
            return Ok(JobOutcome::Done);
        }

        ctx.progress(&job, "extracting_scenes", 0.3).await;
        let scenes = crate::generation::paragraphs::extract_scenes(
            &self.0,
            &UserId(user_raw.clone()),
            &audiobook_id,
            &book.title,
            &book.topic,
            book.genre.as_deref(),
            &chapter.title,
            &paragraphs,
        )
        .await;

        // STEM-only second pass: label paragraphs with diagram
        // templates so the Manim render path knows what to draw.
        // Effective `is_stem` follows the same fallback rule the
        // detail endpoint exposes: override > detected > false.
        let is_stem = book
            .stem_override
            .unwrap_or_else(|| book.stem_detected.unwrap_or(false));
        let visuals = if is_stem {
            ctx.progress(&job, "extracting_visuals", 0.5).await;
            crate::generation::paragraphs::extract_visual_kinds(
                &self.0,
                &UserId(user_raw.clone()),
                &audiobook_id,
                &book.title,
                &book.topic,
                book.genre.as_deref(),
                &chapter.title,
                &paragraphs,
            )
            .await
        } else {
            std::collections::HashMap::new()
        };

        // Phase H — code-gen pass for paragraphs the classifier
        // marked `custom_manim`. No-op when the classifier picked
        // none (almost always — the prompt instructs the model to
        // use the custom escape hatch sparingly).
        let manim_codes: std::collections::HashMap<u32, String> = if is_stem
            && visuals
                .values()
                .any(|v| v.visual_kind == "custom_manim")
        {
            ctx.progress(&job, "generating_manim_code", 0.65).await;
            let kinds_only: std::collections::HashMap<u32, String> = visuals
                .iter()
                .map(|(k, v)| (*k, v.visual_kind.clone()))
                .collect();
            // We don't yet know the per-paragraph audio durations
            // here (audio is rendered chapter-wide later). Use the
            // generation::manim_code default — the publisher floors
            // run_seconds at MIN_RUN_SECONDS anyway.
            let durations = std::collections::HashMap::new();
            let custom = crate::generation::manim_code::custom_paragraphs(
                &paragraphs,
                &kinds_only,
                &durations,
            );
            let codes = crate::generation::manim_code::generate_manim_code(
                &self.0,
                &UserId(user_raw.clone()),
                &audiobook_id,
                &book.title,
                &book.topic,
                book.genre.as_deref(),
                &chapter.title,
                "library",
                &custom,
            )
            .await;
            codes
                .into_iter()
                .map(|(k, v)| (k, v.code))
                .collect()
        } else {
            std::collections::HashMap::new()
        };

        let chapter_id = chapter.id.id.to_raw();
        let merged = crate::generation::paragraphs::merge_for_persist(
            &paragraphs,
            &scenes,
            &visuals,
            &manim_codes,
        );
        if let Err(e) =
            crate::generation::paragraphs::persist(&self.0, &chapter_id, merged).await
        {
            return Ok(JobOutcome::Retry(format!("persist paragraphs: {e}")));
        }

        let visual: Vec<u32> = paragraphs
            .iter()
            .filter(|p| scenes.contains_key(&p.index))
            .map(|p| p.index)
            .collect();
        info!(
            audiobook = %audiobook_id,
            chapter = chapter_number,
            paragraphs = paragraphs.len(),
            visual = visual.len(),
            diagrams = visuals.len(),
            tiles_per_visual = per_paragraph,
            stem = is_stem,
            "chapter_paragraphs: scenes extracted"
        );

        // Fan out one Cover-with-paragraph-payload child per (paragraph,
        // ordinal). Each child has its own retry budget; failures here
        // stay scoped to that tile.
        ctx.progress(&job, "fan_out", 0.7).await;
        let user = UserId(user_raw.clone());
        for paragraph_index in &visual {
            for ordinal in 1..=per_paragraph {
                let req = EnqueueRequest::new(JobKind::Cover)
                    .with_user(user.clone())
                    .with_audiobook(AudiobookId(audiobook_id.clone()))
                    .with_chapter(chapter_number)
                    .with_payload(serde_json::json!({
                        "paragraph_index": paragraph_index,
                        "ordinal": ordinal,
                        "total_ordinals": per_paragraph,
                    }))
                    .with_max_attempts(5);
                if let Err(e) = ctx.repo.enqueue(req).await {
                    warn!(
                        error = %e,
                        audiobook = %audiobook_id,
                        chapter = chapter_number,
                        paragraph = paragraph_index,
                        ordinal,
                        "chapter_paragraphs: enqueue tile failed"
                    );
                }
            }
        }
        publish_job_snapshot(ctx, &audiobook_id).await;
        ctx.progress(&job, "done", 1.0).await;
        Ok(JobOutcome::Done)
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
        publish_job_snapshot(ctx, &audiobook_id).await;

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
                if any_dead {
                    return Ok(JobOutcome::Fatal("one or more chapters failed".into()));
                }
                // Auto-pipeline: kick off the YouTube publish if it was
                // requested at create time. Best-effort — a missing
                // YouTube account just warns and lets the user publish
                // manually from the UI.
                if let Some(pipeline) = load_auto_pipeline(&self.0, &audiobook_id).await {
                    if let Some(publish) = pipeline.publish {
                        enqueue_auto_publish(
                            &self.0,
                            ctx,
                            UserId(user_id.clone()),
                            &audiobook_id,
                            &language,
                            &publish,
                        )
                        .await;
                    }
                }
                return Ok(JobOutcome::Done);
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

/// Load the audiobook's `auto_pipeline` column. Decodes straight into the
/// typed DTO — going through `serde_json::Value` doesn't round-trip
/// reliably through SurrealDB's `option<object>` (inner fields silently
/// default), which is why pre-2026-04 audiobooks never auto-narrated.
async fn load_auto_pipeline(
    state: &AppState,
    audiobook_id: &str,
) -> Option<crate::handlers::audiobook::AutoPipelineRequest> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        auto_pipeline: Option<crate::handlers::audiobook::AutoPipelineRequest>,
    }
    let rows: Vec<Row> = match state
        .db()
        .inner()
        .query(format!(
            "SELECT auto_pipeline FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .and_then(|mut r| r.take(0))
    {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, audiobook_id, "auto-pipeline: load failed");
            return None;
        }
    };
    rows.into_iter().next()?.auto_pipeline
}

/// Enqueue a `Cover` job (with `chapter_number` set) for every chapter
/// in the audiobook's primary language. The Cover handler dispatches on
/// `chapter_number` to run the per-chapter art branch.
async fn enqueue_chapter_art_jobs(
    state: &AppState,
    ctx: &JobContext,
    user_raw: &str,
    audiobook_id: &str,
) -> Result<()> {
    let primary = primary_language(state, audiobook_id).await?;
    #[derive(Deserialize)]
    struct Row {
        number: i64,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT number FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang \
             ORDER BY number ASC"
        ))
        .bind(("lang", primary))
        .await
        .map_err(|e| Error::Database(format!("chapter art enumerate: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("chapter art enumerate (decode): {e}")))?;

    // How many tiles to render per visual paragraph. 0 = chapter cover
    // tiles only; >0 fans out a `ChapterParagraphs` orchestrator per
    // chapter that does the extract LLM pass + per-tile child jobs.
    let per_paragraph = load_images_per_paragraph(state, audiobook_id).await;

    info!(
        audiobook_id,
        chapters = rows.len(),
        per_paragraph,
        "auto-pipeline: enqueueing chapter art jobs"
    );
    // Image gen flakes more than text gen (content-filter false positives,
    // empty payloads, occasional truncation). Give chapter-art jobs a
    // larger retry budget than text jobs so a single transient failure
    // doesn't dead-letter the tile.
    let art_attempts = 5;
    for ch in rows {
        let req = EnqueueRequest::new(JobKind::Cover)
            .with_user(UserId(user_raw.to_string()))
            .with_audiobook(AudiobookId(audiobook_id.to_string()))
            .with_chapter(ch.number as u32)
            .with_max_attempts(art_attempts);
        if let Err(e) = ctx.repo.enqueue(req).await {
            warn!(
                error = %e,
                audiobook_id,
                chapter = ch.number,
                "auto-pipeline: enqueue chapter-art job failed"
            );
        }
        if per_paragraph > 0 {
            let req = EnqueueRequest::new(JobKind::ChapterParagraphs)
                .with_user(UserId(user_raw.to_string()))
                .with_audiobook(AudiobookId(audiobook_id.to_string()))
                .with_chapter(ch.number as u32)
                .with_payload(serde_json::json!({
                    "images_per_paragraph": per_paragraph,
                }))
                // Orchestrator does an LLM extract call + fans out N
                // child Cover jobs. The expensive part (image gen) is
                // owned by the children with their own retry budgets,
                // so the orchestrator itself doesn't need many attempts.
                .with_max_attempts(3);
            if let Err(e) = ctx.repo.enqueue(req).await {
                warn!(
                    error = %e,
                    audiobook_id,
                    chapter = ch.number,
                    "auto-pipeline: enqueue chapter-paragraphs job failed"
                );
            }
        }
    }
    Ok(())
}

async fn load_images_per_paragraph(state: &AppState, audiobook_id: &str) -> u32 {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        images_per_paragraph: Option<i64>,
    }
    let mut resp = match state
        .db()
        .inner()
        .query(format!(
            "SELECT images_per_paragraph FROM audiobook:`{audiobook_id}`"
        ))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, audiobook_id, "load images_per_paragraph failed");
            return 0;
        }
    };
    let rows: Vec<Row> = match resp.take(0) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, audiobook_id, "decode images_per_paragraph failed");
            return 0;
        }
    };
    rows.into_iter()
        .next()
        .and_then(|r| r.images_per_paragraph)
        .map(|n| n.clamp(0, 3) as u32)
        .unwrap_or(0)
}

async fn publish_job_snapshot(ctx: &JobContext, audiobook_id: &str) {
    let jobs = match ctx.repo.list_for_audiobook(audiobook_id).await {
        Ok(j) => j,
        Err(e) => {
            warn!(
                error = %e,
                audiobook_id,
                "progress snapshot after enqueue failed"
            );
            return;
        }
    };
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
    ctx.hub
        .publish(
            audiobook_id,
            ProgressEvent::Snapshot {
                audiobook_id: audiobook_id.to_string(),
                jobs: snapshots,
                at: chrono::Utc::now(),
            },
        )
        .await;
}

/// Mint a publication row + enqueue a `publish_youtube` job. Skips with
/// a warn (instead of a hard fail) when the user has no YouTube account
/// connected — we don't want a successful narration to look broken just
/// because the publish step couldn't run.
async fn enqueue_auto_publish(
    state: &AppState,
    ctx: &JobContext,
    user: UserId,
    audiobook_id: &str,
    language: &str,
    publish: &crate::handlers::audiobook::AutoPublishRequest,
) {
    // Account check.
    #[derive(Deserialize)]
    struct AccountRow {
        #[serde(default)]
        _id: Option<surrealdb::sql::Thing>,
    }
    let account_rows: Vec<AccountRow> = match state
        .db()
        .inner()
        .query(format!(
            "SELECT id FROM youtube_account WHERE owner = user:`{}` LIMIT 1",
            user.0
        ))
        .await
    {
        Ok(mut r) => r.take(0).unwrap_or_default(),
        Err(e) => {
            warn!(error = %e, "auto-pipeline: yt account lookup failed");
            return;
        }
    };
    if account_rows.is_empty() {
        warn!(
            audiobook_id,
            "auto-pipeline: skipping publish — no YouTube channel connected"
        );
        return;
    }

    let mut mode = publish
        .mode
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("single")
        .to_string();
    let privacy = publish
        .privacy_status
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("private")
        .to_string();
    let review = publish.review;

    // Shorts always upload as a single vertical clip — clamp the mode
    // here so a stale `auto_pipeline.publish.mode` from before the
    // Short flag was flipped on can't land the upload in playlist
    // mode. Mirrors the override on the manual publish handler and in
    // the publisher itself.
    if load_audiobook_is_short(state, audiobook_id).await.unwrap_or(false) {
        mode = "single".to_string();
    }

    // Mint a fresh publication row. We don't bother with the upsert
    // path the HTTP handler uses because auto-publish only ever fires
    // once per audiobook (right after the first narration).
    let publication_id = uuid::Uuid::new_v4().simple().to_string();
    let create_sql = format!(
        r#"CREATE youtube_publication:`{publication_id}` CONTENT {{
            audiobook: audiobook:`{audiobook_id}`,
            language: $lang,
            privacy_status: $p,
            mode: $m,
            review: $r
        }}"#
    );
    if let Err(e) = state
        .db()
        .inner()
        .query(create_sql)
        .bind(("lang", language.to_string()))
        .bind(("p", privacy.clone()))
        .bind(("m", mode.clone()))
        .bind(("r", review))
        .await
        .and_then(|r| r.check())
    {
        warn!(
            error = %e,
            audiobook_id,
            "auto-pipeline: create publication row failed"
        );
        return;
    }

    let payload = serde_json::json!({
        "publication_id": publication_id,
        "privacy_status": privacy,
        "mode": mode,
        "review": review,
    });
    let req = EnqueueRequest::new(JobKind::PublishYoutube)
        .with_user(user)
        .with_audiobook(AudiobookId(audiobook_id.to_string()))
        .with_language(language.to_string())
        .with_payload(payload)
        .with_max_attempts(3);
    if let Err(e) = ctx.repo.enqueue(req).await {
        warn!(
            error = %e,
            audiobook_id,
            "auto-pipeline: enqueue publish_youtube failed"
        );
    } else {
        info!(
            audiobook_id,
            publication_id,
            "auto-pipeline: publish_youtube enqueued"
        );
    }
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

/// Read `audiobook.is_short`. Defaults to `false` for rows that pre-date
/// migration 0031 or any DB error — the worst case is a Short that
/// accidentally goes out as a horizontal video, which is recoverable;
/// hard-failing the publish would be worse.
async fn load_audiobook_is_short(state: &AppState, audiobook_id: &str) -> Result<bool> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        is_short: Option<bool>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT is_short FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("auto-publish is_short: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("auto-publish is_short (decode): {e}")))?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.is_short)
        .unwrap_or(false))
}
