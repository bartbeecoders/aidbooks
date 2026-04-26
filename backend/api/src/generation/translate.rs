//! Chapter translation via the OpenRouter LLM.
//!
//! Given an audiobook with a primary-language chapter set, produce a
//! parallel chapter set in a target language by translating each row's
//! `title`, `synopsis`, and `body_md`. New rows share the audiobook + number
//! but carry the target `language`. Audio is generated separately via the
//! existing TTS endpoint with `?language=<>`.

use listenai_core::domain::LlmRole;
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::llm::{pick_model_for_role, ChatMessage, ChatRequest};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct SrcChapter {
    number: i64,
    title: String,
    synopsis: Option<String>,
    target_words: Option<i64>,
    body_md: Option<String>,
}

/// Translate every chapter from `source` to `target`, persisting the result
/// as new chapter rows. Returns the number of chapters created.
///
/// If a chapter row already exists for `(audiobook, number, target)`, it is
/// skipped — the user can delete it and re-run if they want a fresh pass.
pub async fn translate_audiobook(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    source: &str,
    target: &str,
) -> Result<usize> {
    if source == target {
        return Err(Error::Validation(
            "source and target language must differ".into(),
        ));
    }

    // Load the source chapters.
    let chapters: Vec<SrcChapter> = state
        .db()
        .inner()
        .query(format!(
            "SELECT number, title, synopsis, target_words, body_md FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang \
             ORDER BY number ASC"
        ))
        .bind(("lang", source.to_string()))
        .await
        .map_err(|e| Error::Database(format!("translate load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("translate load (decode): {e}")))?;

    if chapters.is_empty() {
        return Err(Error::Validation(format!(
            "no chapters in source language `{source}` to translate"
        )));
    }

    // Skip chapters that already have a translation, so re-running translate
    // doesn't produce duplicates (the unique index would reject them anyway).
    let existing: Vec<i64> = state
        .db()
        .inner()
        .query(format!(
            "SELECT VALUE number FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang"
        ))
        .bind(("lang", target.to_string()))
        .await
        .map_err(|e| Error::Database(format!("translate exists: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("translate exists (decode): {e}")))?;
    let existing_set: std::collections::HashSet<i64> = existing.into_iter().collect();

    let model = pick_model_for_role(state, LlmRole::Chapter).await?;
    let source_label = crate::i18n::label(source);
    let target_label = crate::i18n::label(target);

    let mut created = 0usize;
    for ch in chapters {
        if existing_set.contains(&ch.number) {
            info!(
                audiobook = audiobook_id,
                chapter = ch.number,
                target,
                "translation already exists, skipping"
            );
            continue;
        }
        let body = ch.body_md.clone().unwrap_or_default();
        if body.trim().is_empty() {
            warn!(
                audiobook = audiobook_id,
                chapter = ch.number,
                "skipping translation: source body_md is empty"
            );
            continue;
        }

        let translated_title = call_translate(state, &model, source_label, target_label, &ch.title).await?;
        let translated_synopsis = match ch.synopsis.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(s) => Some(call_translate(state, &model, source_label, target_label, s).await?),
            None => None,
        };
        let translated_body =
            call_translate_prose(state, &model, source_label, target_label, &body).await?;

        let ch_id = uuid::Uuid::new_v4().simple().to_string();
        state
            .db()
            .inner()
            .query(format!(
                r#"CREATE chapter:`{ch_id}` CONTENT {{
                    audiobook: audiobook:`{audiobook_id}`,
                    number: $number,
                    title: $title,
                    synopsis: $synopsis,
                    target_words: $target_words,
                    body_md: $body_md,
                    status: "text_ready",
                    language: $language
                }}"#
            ))
            .bind(("number", ch.number))
            .bind(("title", translated_title))
            .bind(("synopsis", translated_synopsis))
            .bind((
                "target_words",
                ch.target_words.unwrap_or(1200),
            ))
            .bind(("body_md", translated_body))
            .bind(("language", target.to_string()))
            .await
            .map_err(|e| Error::Database(format!("create translated chapter: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("create translated chapter: {e}")))?;
        created += 1;

        // Best-effort progress log.
        let _ = user;
        info!(
            audiobook = audiobook_id,
            chapter = ch.number,
            target,
            "chapter translated"
        );
    }

    Ok(created)
}

async fn call_translate(
    state: &AppState,
    model: &str,
    source: &str,
    target: &str,
    text: &str,
) -> Result<String> {
    let req = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::system(format!(
                "You are a literary translator. Translate the user's text from {source} \
                 to {target}. Preserve meaning, tone, and proper nouns. Reply with the \
                 translation only — no preamble, no quotes, no commentary."
            )),
            ChatMessage::user(text.to_string()),
        ],
        temperature: Some(0.3),
        max_tokens: Some(500),
        json_mode: None,
        modalities: None,
    };
    let resp = state.llm().chat(&req).await?;
    Ok(resp.content.trim().to_string())
}

async fn call_translate_prose(
    state: &AppState,
    model: &str,
    source: &str,
    target: &str,
    text: &str,
) -> Result<String> {
    // For the long body we ask the model to return JSON `{"translation": "..."}`
    // so quote-stripping isn't needed and a stray "Here is the translation:"
    // prefix can't slip through.
    let req = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::system(format!(
                "You are a literary translator. Translate the user's prose from {source} \
                 to {target}. Preserve paragraph breaks, tone, and pacing. Translate \
                 idioms naturally rather than literally. Output a single JSON object \
                 of the form {{\"translation\": \"...\"}} and nothing else."
            )),
            ChatMessage::user(text.to_string()),
        ],
        temperature: Some(0.4),
        max_tokens: Some(8_000),
        json_mode: Some(true),
        modalities: None,
    };
    let resp = state.llm().chat(&req).await?;
    parse_translation(&resp.content)
}

fn parse_translation(content: &str) -> Result<String> {
    let trimmed = content.trim();
    let stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_end_matches("```").trim())
        .unwrap_or(trimmed);
    let v: serde_json::Value = serde_json::from_str(stripped)
        .map_err(|e| Error::Upstream(format!("translate json: {e}")))?;
    Ok(v.get("translation")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Upstream("translate: missing `translation` field".into()))?
        .to_string())
}

// silences `unused` warning on json! when we add structured logging later.
#[allow(dead_code)]
fn _keep_json_used() -> serde_json::Value {
    json!({})
}
