//! Outline generation.
//!
//! Takes a topic + length + genre, calls the outline LLM with the
//! `outline` prompt template, parses the JSON result, and writes the
//! resulting title + chapter rows into the DB. The audiobook moves through
//! `outline_pending` → `outline_ready` (or `failed`) during this call.

use std::collections::HashMap;

use listenai_core::domain::{AudiobookLength, LlmRole, PromptRole};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use tracing::{info, warn};

use crate::generation::prompts;
use crate::llm::{pick_llm_for_roles_lang, ChatMessage, ChatRequest};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct OutlineJson {
    title: String,
    #[serde(default)]
    #[allow(dead_code)]
    subtitle: String,
    /// X.ai TTS speech-tag palette the chapter writer should embed inline.
    /// Filtered to the supported set in `sanitize_tags` before persisting.
    #[serde(default)]
    tags: Vec<String>,
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
///
/// `is_short`: when `true`, override the chapter count + per-chapter word
/// budget so the result fits inside a 90-second YouTube Short — a single
/// chapter capped at 225 words (~1.5 minutes of narration at 150 wpm).
/// The `length` preset is ignored in that case.
pub async fn run(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    topic: &str,
    length: AudiobookLength,
    genre: &str,
    language: &str,
    is_short: bool,
) -> Result<()> {
    set_audiobook_status(state, audiobook_id, "outline_pending").await?;

    // Shorts: one tiny chapter, ≤ 90 s of narration. 150 wpm × 1.5 min
    // ≈ 225 words. Keeps the budget well under YouTube's 60 s Short
    // baseline even at slower TTS pacing.
    let (chapter_count, words_per_chapter) = if is_short {
        (1u32, SHORT_WORDS_PER_CHAPTER)
    } else {
        (length.chapter_count(), length.words_per_chapter())
    };

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("topic", topic.to_string());
    vars.insert(
        "length",
        if is_short {
            "short_form".to_string()
        } else {
            format!("{length:?}").to_lowercase()
        },
    );
    vars.insert("genre", genre.to_string());
    vars.insert("chapter_count", chapter_count.to_string());
    vars.insert("words_per_chapter", words_per_chapter.to_string());
    vars.insert("language", crate::i18n::label(language).to_string());

    let rendered = prompts::render(state, PromptRole::Outline, &vars).await?;
    // Honor the admin's `default_for: ["outline"]` + `priority` ranking,
    // and prefer rows whose `languages` includes the book's language. Falls
    // back to `Config.openrouter_default_model` only when no row matches.
    let picked = pick_llm_for_roles_lang(
        state,
        &[LlmRole::Outline],
        Some(language),
    )
    .await?;

    let req = ChatRequest {
        model: picked.model_id.clone(),
        messages: vec![
            ChatMessage::system("You write audiobook outlines as strict JSON."),
            ChatMessage::user(rendered.body),
        ],
        temperature: Some(0.7),
        max_tokens: Some(2_000),
        json_mode: Some(true),
        modalities: None,
        provider: Some(picked.provider.clone()),
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
        &picked.llm_id,
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

    persist_outline(state, audiobook_id, &outline, length, language, is_short).await?;
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

/// Filter LLM-suggested speech tags down to the X.ai-supported set, dedupe
/// in input order, and cap length. Anything not on the allowlist is dropped
/// silently — the chapter prompt still works without tags, so a sloppy
/// outline shouldn't abort the run.
fn sanitize_tags(raw: &[String]) -> Vec<String> {
    const ALLOWED: &[&str] = &[
        // Inline (single-point).
        "[pause]",
        "[long-pause]",
        "[laugh]",
        "[cry]",
        "[cough]",
        "[throat-clear]",
        "[inhale]",
        "[exhale]",
        // Wrapping — store the opening form only; the chapter writer pairs
        // it with the matching `</tag>` itself.
        "<soft>",
        "<loud>",
        "<high>",
        "<low>",
        "<fast>",
        "<slow>",
        "<whisper>",
        "<singing>",
    ];
    let mut out: Vec<String> = Vec::new();
    for raw_tag in raw {
        let t = raw_tag.trim().to_lowercase();
        if t.is_empty() {
            continue;
        }
        // Tolerate models that returned a closing wrapper form.
        let normalized = if let Some(inner) = t.strip_prefix("</").and_then(|s| s.strip_suffix('>')) {
            format!("<{inner}>")
        } else {
            t
        };
        if ALLOWED.contains(&normalized.as_str()) && !out.contains(&normalized) {
            out.push(normalized);
        }
        if out.len() >= 12 {
            break;
        }
    }
    out
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

/// Word budget used for YouTube Short outlines. ~150 wpm × 1.5 minutes.
/// Kept private — callers go through `run(..., is_short = true)`.
const SHORT_WORDS_PER_CHAPTER: u32 = 225;

async fn persist_outline(
    state: &AppState,
    audiobook_id: &str,
    outline: &OutlineJson,
    length: AudiobookLength,
    language: &str,
    is_short: bool,
) -> Result<()> {
    let tags = sanitize_tags(&outline.tags);
    // Replace title + speech-tag palette in one round-trip.
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook:`{audiobook_id}` SET title = $title, tags = $tags"
        ))
        .bind(("title", outline.title.clone()))
        .bind(("tags", tags))
        .await
        .map_err(|e| Error::Database(format!("persist title: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("persist title: {e}")))?;

    // Wipe and recreate the primary-language chapters for this audiobook
    // so regeneration is clean. Scoping by language preserves any
    // translation rows that already exist for other languages.
    state
        .db()
        .inner()
        .query(format!(
            "DELETE chapter WHERE audiobook = audiobook:`{audiobook_id}` \
             AND language = $lang"
        ))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("wipe chapters: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("wipe chapters: {e}")))?;

    for ch in &outline.chapters {
        let ch_id = uuid::Uuid::new_v4().simple().to_string();
        // Without `language`, SurrealDB applies the field default (`"en"`,
        // see 0007_chapter_lang.surql) — which would orphan a Dutch book's
        // chapters under "en". Bind explicitly.
        let sql = format!(
            r#"CREATE chapter:`{ch_id}` CONTENT {{
                audiobook: audiobook:`{audiobook_id}`,
                number: $number,
                title: $title,
                synopsis: $synopsis,
                target_words: $target_words,
                status: "pending",
                language: $language
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
                ch.target_words.unwrap_or_else(|| {
                    if is_short {
                        SHORT_WORDS_PER_CHAPTER
                    } else {
                        length.words_per_chapter()
                    }
                }) as i64,
            ))
            .bind(("language", language.to_string()))
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
        PromptRole::Cover => "cover",
        PromptRole::ParagraphImage => "paragraph_image",
        PromptRole::Translate => "translate",
        PromptRole::SceneExtract => "scene_extract",
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
    // Cost resolution priority:
    //   1. Mock path → $0 (no real billing happened).
    //   2. Upstream populated `usage.cost` (OpenRouter when
    //      `usage:{include:true}` was sent) → use it as-is.
    //   3. Otherwise → compute from the LLM row's per-1k pricing × token
    //      counts. xAI's chat API doesn't return a billed cost, so this
    //      branch is what makes Grok costs show up in the badge.
    let cost = if response.mocked {
        0.0
    } else if response.usage.cost > 0.0 {
        response.usage.cost
    } else {
        compute_cost_from_row(state, llm_id, &response.usage)
            .await
            .unwrap_or(0.0)
    };
    state
        .db()
        .inner()
        .query(sql)
        .bind(("role", role_str.to_string()))
        .bind(("pt", response.usage.prompt_tokens as i64))
        .bind(("ct", response.usage.completion_tokens as i64))
        .bind(("cost", cost))
        .bind(("success", success))
        .bind(("error", error.map(|s| s.to_string())))
        .await
        .map_err(|e| Error::Database(format!("log generation event: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("log generation event: {e}")))?;
    Ok(())
}

/// Compute USD cost from the LLM row's per-1k pricing × the response's
/// token counts. Used when the upstream didn't return a `usage.cost`
/// field — most notably xAI, which only ships token counts. Returns
/// `None` if the row can't be loaded (e.g. `_default_` placeholder); the
/// caller falls back to `0.0` in that case.
async fn compute_cost_from_row(
    state: &AppState,
    llm_id: &str,
    usage: &crate::llm::ChatUsage,
) -> Option<f64> {
    if !is_safe_llm_id(llm_id) {
        return None;
    }
    #[derive(Deserialize)]
    struct PriceRow {
        #[serde(default)]
        cost_prompt_per_1k: f64,
        #[serde(default)]
        cost_completion_per_1k: f64,
    }
    let rows: Vec<PriceRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT cost_prompt_per_1k, cost_completion_per_1k \
             FROM llm:`{llm_id}`"
        ))
        .await
        .ok()?
        .take(0)
        .ok()?;
    let row = rows.into_iter().next()?;
    let pt = usage.prompt_tokens as f64;
    let ct = usage.completion_tokens as f64;
    Some(pt * row.cost_prompt_per_1k / 1000.0 + ct * row.cost_completion_per_1k / 1000.0)
}

/// Same charset rule used elsewhere — keeps the embedded `llm:<id>` safe
/// from injection. Duplicated here to avoid coupling with cover.rs.
fn is_safe_llm_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
