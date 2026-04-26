//! Outline generation.
//!
//! Takes a topic + length + genre, calls the outline LLM with the
//! `outline` prompt template, parses the JSON result, and writes the
//! resulting title + chapter rows into the DB. The audiobook moves through
//! `outline_pending` → `outline_ready` (or `failed`) during this call.

use std::collections::HashMap;

use listenai_core::domain::{AudiobookLength, PromptRole};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use tracing::{info, warn};

use crate::generation::prompts;
use crate::llm::{ChatMessage, ChatRequest};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct OutlineJson {
    title: String,
    #[serde(default)]
    #[allow(dead_code)]
    subtitle: String,
    chapters: Vec<OutlineChapter>,
}

#[derive(Debug, Deserialize)]
struct OutlineChapter {
    number: u32,
    title: String,
    #[serde(default)]
    synopsis: Option<String>,
    #[serde(default)]
    target_words: Option<u32>,
}

const OUTLINE_MODEL_FALLBACK_HINT: &str = "outline LLM produced invalid JSON";

/// Run the outline step. Mutates `audiobook:<id>` with title and the
/// generated chapter rows on success.
pub async fn run(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    topic: &str,
    length: AudiobookLength,
    genre: &str,
    language: &str,
) -> Result<()> {
    set_audiobook_status(state, audiobook_id, "outline_pending").await?;

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("topic", topic.to_string());
    vars.insert("length", format!("{length:?}").to_lowercase());
    vars.insert("genre", genre.to_string());
    vars.insert("chapter_count", length.chapter_count().to_string());
    vars.insert("words_per_chapter", length.words_per_chapter().to_string());
    vars.insert("language", crate::i18n::label(language).to_string());

    let rendered = prompts::render(state, PromptRole::Outline, &vars).await?;
    let model = state.config().openrouter_default_model.clone();

    let req = ChatRequest {
        model,
        messages: vec![
            ChatMessage::system("You write audiobook outlines as strict JSON."),
            ChatMessage::user(rendered.body),
        ],
        temperature: Some(0.7),
        max_tokens: Some(2_000),
        json_mode: Some(true),
        modalities: None,
    };

    let response = match state.llm().chat(&req).await {
        Ok(r) => r,
        Err(e) => {
            set_audiobook_status(state, audiobook_id, "failed")
                .await
                .ok();
            return Err(e);
        }
    };

    log_generation_event(
        state,
        user,
        Some(audiobook_id),
        "claude_haiku_4_5",
        PromptRole::Outline,
        &response,
        None,
    )
    .await?;

    let outline = match parse_outline(&response.content) {
        Ok(o) => o,
        Err(msg) => {
            warn!(content_preview = %truncate(&response.content, 300), "{OUTLINE_MODEL_FALLBACK_HINT}");
            set_audiobook_status(state, audiobook_id, "failed")
                .await
                .ok();
            return Err(Error::Upstream(format!("outline parse: {msg}")));
        }
    };

    persist_outline(state, audiobook_id, &outline, length).await?;
    set_audiobook_status(state, audiobook_id, "outline_ready").await?;

    info!(
        audiobook = audiobook_id,
        title = %outline.title,
        chapters = outline.chapters.len(),
        mocked = response.mocked,
        "outline ready"
    );
    Ok(())
}

fn parse_outline(content: &str) -> std::result::Result<OutlineJson, String> {
    // Some models wrap JSON in ```json ...``` despite being asked not to.
    let cleaned = strip_code_fences(content);
    serde_json::from_str::<OutlineJson>(cleaned).map_err(|e| e.to_string())
}

fn strip_code_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

async fn set_audiobook_status(state: &AppState, audiobook_id: &str, status: &str) -> Result<()> {
    let sql = format!("UPDATE audiobook:`{audiobook_id}` SET status = $status");
    state
        .db()
        .inner()
        .query(sql)
        .bind(("status", status.to_string()))
        .await
        .map_err(|e| Error::Database(format!("set status: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set status: {e}")))?;
    Ok(())
}

async fn persist_outline(
    state: &AppState,
    audiobook_id: &str,
    outline: &OutlineJson,
    length: AudiobookLength,
) -> Result<()> {
    // Replace title.
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook:`{audiobook_id}` SET title = $title"
        ))
        .bind(("title", outline.title.clone()))
        .await
        .map_err(|e| Error::Database(format!("persist title: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("persist title: {e}")))?;

    // Wipe and recreate chapters for this audiobook so regeneration is clean.
    state
        .db()
        .inner()
        .query(format!(
            "DELETE chapter WHERE audiobook = audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("wipe chapters: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("wipe chapters: {e}")))?;

    for ch in &outline.chapters {
        let ch_id = uuid::Uuid::new_v4().simple().to_string();
        let sql = format!(
            r#"CREATE chapter:`{ch_id}` CONTENT {{
                audiobook: audiobook:`{audiobook_id}`,
                number: $number,
                title: $title,
                synopsis: $synopsis,
                target_words: $target_words,
                status: "pending"
            }}"#
        );
        state
            .db()
            .inner()
            .query(sql)
            .bind(("number", ch.number as i64))
            .bind(("title", ch.title.clone()))
            .bind(("synopsis", ch.synopsis.clone()))
            .bind((
                "target_words",
                ch.target_words
                    .unwrap_or_else(|| length.words_per_chapter()) as i64,
            ))
            .await
            .map_err(|e| Error::Database(format!("create chapter: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("create chapter: {e}")))?;
    }
    Ok(())
}

/// Helper used by outline + chapter generation to append a row to
/// `generation_event`.
pub async fn log_generation_event(
    state: &AppState,
    user: &UserId,
    audiobook_id: Option<&str>,
    llm_id: &str,
    role: PromptRole,
    response: &crate::llm::ChatResponse,
    error: Option<&str>,
) -> Result<()> {
    let event_id = uuid::Uuid::new_v4().simple().to_string();
    let audiobook_set = match audiobook_id {
        Some(id) => format!(", audiobook: audiobook:`{id}`"),
        None => String::new(),
    };
    let role_str = match role {
        PromptRole::Outline => "outline",
        PromptRole::Chapter => "chapter",
        PromptRole::RandomTopic => "random_topic",
        PromptRole::Moderation => "moderation",
        PromptRole::Title => "title",
    };
    let sql = format!(
        r#"CREATE generation_event:`{event_id}` CONTENT {{
            user: user:`{user}`,
            llm: llm:`{llm_id}`,
            role: $role,
            prompt_tokens: $pt,
            completion_tokens: $ct,
            cost_usd: $cost,
            success: $success,
            error: $error
            {audiobook_set}
        }}"#,
        user = user.0,
    );
    let success = error.is_none();
    state
        .db()
        .inner()
        .query(sql)
        .bind(("role", role_str.to_string()))
        .bind(("pt", response.usage.prompt_tokens as i64))
        .bind(("ct", response.usage.completion_tokens as i64))
        .bind(("cost", 0.0_f64))
        .bind(("success", success))
        .bind(("error", error.map(|s| s.to_string())))
        .await
        .map_err(|e| Error::Database(format!("log generation event: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("log generation event: {e}")))?;
    Ok(())
}
