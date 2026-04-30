//! Chapter body generation.
//!
//! Runs per chapter: loads the latest outline row, renders the `chapter`
//! prompt, calls the LLM, stores the prose in `chapter.body_md`, and
//! marks the chapter `text_ready`. The caller decides whether to run this
//! sequentially or in parallel — Phase 5 will own the scheduling.

use std::collections::HashMap;

use listenai_core::domain::{LlmRole, PromptRole};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use tracing::info;

use crate::generation::{outline::log_generation_event, prompts};
use crate::llm::{pick_llm_for_roles_lang, ChatMessage, ChatRequest};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct ChapterRow {
    id: surrealdb::sql::Thing,
    number: i64,
    title: String,
    synopsis: Option<String>,
    target_words: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AudiobookMini {
    title: String,
    genre: Option<String>,
    #[serde(default)]
    language: Option<String>,
}

/// Generate every `pending` (or failed) chapter in order. Updates each
/// chapter's `status` and `body_md`, then flips the audiobook to
/// `text_ready` or `failed` when done.
pub async fn run_all(state: &AppState, user: &UserId, audiobook_id: &str) -> Result<()> {
    set_audiobook_status(state, audiobook_id, "chapters_running").await?;

    let book = load_audiobook(state, audiobook_id).await?;
    let lang = book.language.as_deref().unwrap_or("en");
    let chapters = load_chapters(state, audiobook_id, lang).await?;

    let mut previous_ending: Option<String> = None;
    let mut any_failed = false;

    for ch in chapters {
        match run_one(
            state,
            user,
            audiobook_id,
            &book,
            &ch,
            previous_ending.as_deref().unwrap_or(""),
        )
        .await
        {
            Ok(body) => {
                previous_ending = Some(tail(&body, 400));
            }
            Err(e) => {
                tracing::error!(
                    audiobook = audiobook_id,
                    chapter = ch.number,
                    error = %e,
                    "chapter failed"
                );
                any_failed = true;
            }
        }
    }

    if any_failed {
        set_audiobook_status(state, audiobook_id, "failed").await?;
    } else {
        set_audiobook_status(state, audiobook_id, "text_ready").await?;
    }
    Ok(())
}

/// Regenerate a single chapter by number, ignoring current status. Returns
/// the new body on success.
pub async fn run_one_by_number(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    chapter_number: i64,
) -> Result<String> {
    let book = load_audiobook(state, audiobook_id).await?;
    let lang = book.language.as_deref().unwrap_or("en");
    let chapters = load_chapters(state, audiobook_id, lang).await?;
    let ch = chapters
        .iter()
        .find(|c| c.number == chapter_number)
        .ok_or(Error::NotFound {
            resource: format!("audiobook:{audiobook_id} chapter {chapter_number}"),
        })?;
    let prev = previous_body(state, audiobook_id, chapter_number, lang).await?;
    run_one(state, user, audiobook_id, &book, ch, &prev).await
}

async fn run_one(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    book: &AudiobookMini,
    ch: &ChapterRow,
    previous_ending: &str,
) -> Result<String> {
    let chapter_raw_id = ch.id.id.to_raw();
    set_chapter_status(state, &chapter_raw_id, "running").await?;

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("book_title", book.title.clone());
    vars.insert("genre", book.genre.clone().unwrap_or_default());
    vars.insert("chapter_number", ch.number.to_string());
    vars.insert("chapter_title", ch.title.clone());
    vars.insert("chapter_synopsis", ch.synopsis.clone().unwrap_or_default());
    vars.insert("target_words", ch.target_words.unwrap_or(1200).to_string());
    vars.insert(
        "language",
        crate::i18n::label(book.language.as_deref().unwrap_or("en")).to_string(),
    );
    vars.insert(
        "previous_ending",
        if previous_ending.is_empty() {
            "(this is the first chapter)".into()
        } else {
            previous_ending.to_string()
        },
    );

    let rendered = prompts::render(state, PromptRole::Chapter, &vars).await?;
    // Honor the admin's `default_for: ["chapter"]` + `priority` ranking and
    // language preference. Falls back to `Config.openrouter_default_model`
    // only when no row matches.
    let picked = pick_llm_for_roles_lang(
        state,
        &[LlmRole::Chapter],
        book.language.as_deref(),
    )
    .await?;

    let req = ChatRequest {
        model: picked.model_id.clone(),
        messages: vec![
            ChatMessage::system(
                "You write audiobook chapter prose. Output plain markdown prose only.",
            ),
            ChatMessage::user(rendered.body),
        ],
        temperature: Some(0.8),
        max_tokens: Some(4_000),
        json_mode: Some(false),
        modalities: None,
        provider: Some(picked.provider.clone()),
    };

    let response = match state.llm().chat(&req).await {
        Ok(r) => r,
        Err(e) => {
            set_chapter_status(state, &chapter_raw_id, "failed")
                .await
                .ok();
            log_generation_event(
                state,
                user,
                Some(audiobook_id),
                &picked.llm_id,
                PromptRole::Chapter,
                &crate::llm::ChatResponse {
                    content: String::new(),
                    image_base64: None,
                    usage: Default::default(),
                    mocked: false,
                },
                Some(&e.to_string()),
            )
            .await
            .ok();
            return Err(e);
        }
    };

    log_generation_event(
        state,
        user,
        Some(audiobook_id),
        &picked.llm_id,
        PromptRole::Chapter,
        &response,
        None,
    )
    .await?;

    let body = response.content.trim().to_string();
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE chapter:`{chapter_raw_id}` SET body_md = $body, status = \"text_ready\""
        ))
        .bind(("body", body.clone()))
        .await
        .map_err(|e| Error::Database(format!("persist chapter body: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("persist chapter body: {e}")))?;

    info!(
        audiobook = audiobook_id,
        chapter = ch.number,
        mocked = response.mocked,
        bytes = body.len(),
        "chapter ready"
    );
    Ok(body)
}

async fn load_audiobook(state: &AppState, audiobook_id: &str) -> Result<AudiobookMini> {
    let rows: Vec<AudiobookMini> = state
        .db()
        .inner()
        .query(format!(
            "SELECT title, genre, language FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("load audiobook: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load audiobook (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{audiobook_id}"),
    })
}

async fn load_chapters(
    state: &AppState,
    audiobook_id: &str,
    language: &str,
) -> Result<Vec<ChapterRow>> {
    let rows: Vec<ChapterRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, number, title, synopsis, target_words FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang \
             ORDER BY number ASC"
        ))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("load chapters: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load chapters (decode): {e}")))?;
    Ok(rows)
}

async fn previous_body(
    state: &AppState,
    audiobook_id: &str,
    chapter_number: i64,
    language: &str,
) -> Result<String> {
    let prev = chapter_number - 1;
    if prev < 1 {
        return Ok(String::new());
    }
    let rows: Vec<String> = state
        .db()
        .inner()
        .query(format!(
            "SELECT VALUE body_md FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", prev))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("load prev body: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load prev body (decode): {e}")))?;
    Ok(rows
        .into_iter()
        .next()
        .map(|s| tail(&s, 400))
        .unwrap_or_default())
}

async fn set_audiobook_status(state: &AppState, audiobook_id: &str, status: &str) -> Result<()> {
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

async fn set_chapter_status(state: &AppState, chapter_raw_id: &str, status: &str) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE chapter:`{chapter_raw_id}` SET status = $status"
        ))
        .bind(("status", status.to_string()))
        .await
        .map_err(|e| Error::Database(format!("set chapter status: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set chapter status: {e}")))?;
    Ok(())
}

/// Return the last `n` characters of `s`, respecting UTF-8 char boundaries.
fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut start = s.len() - n;
    while !s.is_char_boundary(start) {
        start -= 1;
    }
    s[start..].to_string()
}
