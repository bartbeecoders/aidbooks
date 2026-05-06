//! Chapter translation via the OpenRouter LLM.
//!
//! Given an audiobook with a primary-language chapter set, produce a
//! parallel chapter set in a target language by translating each row's
//! `title`, `synopsis`, and `body_md`. New rows share the audiobook + number
//! but carry the target `language`. Audio is generated separately via the
//! existing TTS endpoint with `?language=<>`.

use listenai_core::domain::{LlmRole, PromptRole};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::generation::outline::log_generation_event;
use crate::llm::{pick_llm_for_roles_lang, ChatMessage, ChatRequest};
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

    // Prefer a translation-tagged model for the target language; fall back
    // to whatever's serving the chapter role so existing setups (no
    // dedicated translate row) keep working.
    let picked =
        pick_llm_for_roles_lang(state, &[LlmRole::Translate, LlmRole::Chapter], Some(target))
            .await?;
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

        let translated_title = call_translate(
            state,
            user,
            audiobook_id,
            &picked.llm_id,
            &picked.provider,
            &picked.model_id,
            source_label,
            target_label,
            &ch.title,
        )
        .await?;
        let translated_synopsis = match ch.synopsis.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(s) => Some(
                call_translate(
                    state,
                    user,
                    audiobook_id,
                    &picked.llm_id,
                    &picked.provider,
                    &picked.model_id,
                    source_label,
                    target_label,
                    s,
                )
                .await?,
            ),
            None => None,
        };
        let translated_body = call_translate_prose(
            state,
            user,
            audiobook_id,
            &picked.llm_id,
            &picked.provider,
            &picked.model_id,
            source_label,
            target_label,
            &body,
        )
        .await?;

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
            .bind(("target_words", ch.target_words.unwrap_or(1200)))
            .bind(("body_md", translated_body))
            .bind(("language", target.to_string()))
            .await
            .map_err(|e| Error::Database(format!("create translated chapter: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("create translated chapter: {e}")))?;
        created += 1;

        info!(
            audiobook = audiobook_id,
            chapter = ch.number,
            target,
            "chapter translated"
        );
    }

    Ok(created)
}

#[allow(clippy::too_many_arguments)]
async fn call_translate(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    llm_id: &str,
    provider: &str,
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
        provider: Some(provider.to_string()),
    };
    let resp = state.llm().chat(&req).await?;
    log_generation_event(
        state,
        user,
        Some(audiobook_id),
        llm_id,
        PromptRole::Translate,
        &resp,
        None,
    )
    .await
    .ok();
    Ok(resp.content.trim().to_string())
}

#[allow(clippy::too_many_arguments)]
async fn call_translate_prose(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    llm_id: &str,
    provider: &str,
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
        provider: Some(provider.to_string()),
    };
    let resp = state.llm().chat(&req).await?;
    log_generation_event(
        state,
        user,
        Some(audiobook_id),
        llm_id,
        PromptRole::Translate,
        &resp,
        None,
    )
    .await
    .ok();
    parse_translation(&resp.content)
}

/// Pull the translation string out of whatever the LLM actually returned.
///
/// The model is asked for `{"translation": "..."}` with `json_mode = true`,
/// but real-world responses still include any of:
///   * Markdown code fences around the object.
///   * Prose before/after the object ("Here is the translation: …").
///   * Literal newlines inside the string (technically invalid JSON).
///   * Truncated output if max_tokens hits mid-string.
///
/// So we try, in order:
///   1. Strip code fences and parse strict JSON.
///   2. Extract the first balanced `{…}` substring and parse that.
///   3. Apply the same to a newline-escaped copy.
///   4. Pull `translation` via a tolerant regex over the raw text.
///   5. Give up on the JSON contract and return the raw content trimmed.
fn parse_translation(content: &str) -> Result<String> {
    let trimmed = content.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_start().trim_end_matches("```").trim())
        .unwrap_or(trimmed);

    if let Some(s) = parse_strict(unfenced) {
        return Ok(s);
    }
    if let Some(obj) = extract_balanced_object(unfenced) {
        if let Some(s) = parse_strict(obj) {
            return Ok(s);
        }
        let escaped = escape_unescaped_newlines_in_strings(obj);
        if let Some(s) = parse_strict(&escaped) {
            return Ok(s);
        }
    }
    if let Some(s) = extract_translation_via_regex(unfenced) {
        return Ok(s);
    }

    // Last resort: assume the model ignored the JSON contract entirely and
    // gave us the prose directly. Better a slightly-wrapped translation
    // than a hard failure that wastes the user's request.
    let preview: String = content.chars().take(200).collect();
    warn!(
        sample = %preview,
        "translate: response was not parseable as JSON; using raw text"
    );
    Ok(unfenced.to_string())
}

fn parse_strict(s: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    v.get("translation")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Walk the bytes from the first `{` and return the slice up to (and
/// including) the matching `}`. Tracks string state so braces inside
/// quoted text don't fool the depth counter.
fn extract_balanced_object(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_string => escape = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Replace bare `\n`/`\r` characters that appear *inside* a JSON string
/// with their `\n`/`\r` escape sequences. Some models emit literal
/// newlines mid-string, which strict JSON forbids.
fn escape_unescaped_newlines_in_strings(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut in_string = false;
    let mut escape = false;
    for ch in s.chars() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => {
                out.push(ch);
                escape = true;
            }
            '"' => {
                out.push(ch);
                in_string = !in_string;
            }
            '\n' if in_string => out.push_str("\\n"),
            '\r' if in_string => out.push_str("\\r"),
            '\t' if in_string => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/// Last-ditch tolerant extractor — finds the `"translation"` key and
/// reads the quoted value, honouring backslash escapes. Doesn't validate
/// the rest of the document.
fn extract_translation_via_regex(s: &str) -> Option<String> {
    let key_pos = s.find("\"translation\"")?;
    let after_key = &s[key_pos + "\"translation\"".len()..];
    let colon_pos = after_key.find(':')?;
    let after_colon = after_key[colon_pos + 1..].trim_start();
    let bytes = after_colon.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut escape = false;
    for &b in &bytes[1..] {
        if escape {
            // Honour the common escape sequences; drop unknown ones.
            match b {
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'/' => out.push('/'),
                _ => out.push(b as char),
            }
            escape = false;
            continue;
        }
        if b == b'\\' {
            escape = true;
            continue;
        }
        if b == b'"' {
            return Some(out);
        }
        // Multi-byte UTF-8 sequences: bytes >= 0x80 are part of an
        // existing char that already landed via the `for &b` over the
        // string's bytes. Push them back as-is — the whole string is
        // valid UTF-8 because `s` was.
        out.push(b as char);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_object() {
        assert_eq!(
            parse_translation(r#"{"translation":"hola"}"#).unwrap(),
            "hola"
        );
    }

    #[test]
    fn strips_code_fences() {
        let input = "```json\n{\"translation\":\"bonjour\"}\n```";
        assert_eq!(parse_translation(input).unwrap(), "bonjour");
    }

    #[test]
    fn ignores_trailing_prose() {
        let input = "{\"translation\":\"guten tag\"}\n\nLet me know if you'd like another version.";
        assert_eq!(parse_translation(input).unwrap(), "guten tag");
    }

    #[test]
    fn handles_unescaped_newlines_in_string() {
        let input = "{\"translation\":\"line one\nline two\"}";
        assert_eq!(parse_translation(input).unwrap(), "line one\nline two");
    }

    #[test]
    fn falls_back_to_regex_when_object_malformed() {
        // Trailing junk after the value, no closing brace.
        let input = "{\"translation\":\"ciao\" oops more text";
        assert_eq!(parse_translation(input).unwrap(), "ciao");
    }
}

// silences `unused` warning on json! when we add structured logging later.
#[allow(dead_code)]
fn _keep_json_used() -> serde_json::Value {
    json!({})
}
