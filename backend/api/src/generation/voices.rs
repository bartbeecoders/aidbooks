//! Multi-voice narration: split chapter prose into role-tagged
//! segments so a TTS pipeline can render each segment with the role's
//! mapped voice.
//!
//! One LLM call per chapter. The result is cached on
//! `chapter.voice_segments` so re-narrating the chapter (after a
//! voice swap, for example) doesn't pay for the extract again. The
//! cache is invalidated on translate (translated chapters get fresh
//! prose, so the segmentation has to re-run).
//!
//! Roles emitted: `narrator`, `dialogue_male`, `dialogue_female`. The
//! audio pipeline maps each role to a `voice:<id>` via the
//! audiobook's `voice_roles` field, with `narrator` as the fallback
//! whenever a role isn't mapped.

use std::collections::HashMap;

use listenai_core::domain::{LlmRole, PromptRole};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::generation::outline::log_generation_event;
use crate::llm::{pick_llm_for_roles_lang, ChatMessage, ChatRequest};
use crate::state::AppState;

/// Canonical role strings the LLM must emit. Anything else is
/// coerced to `narrator` by [`canonical_role`].
pub const ROLE_NARRATOR: &str = "narrator";
pub const ROLE_DIALOGUE_MALE: &str = "dialogue_male";
pub const ROLE_DIALOGUE_FEMALE: &str = "dialogue_female";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceSegment {
    pub role: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct ExtractResponse {
    #[serde(default)]
    segments: Vec<VoiceSegment>,
}

/// Map any inbound role string onto one of the canonical three. Keeps
/// the audio pipeline tolerant of LLM quirks (capitalisation, extra
/// underscores, unexpected role names) without dropping the segment.
pub fn canonical_role(s: &str) -> &'static str {
    match s.trim().to_ascii_lowercase().as_str() {
        "dialogue_male" | "male" | "dialogue-male" => ROLE_DIALOGUE_MALE,
        "dialogue_female" | "female" | "dialogue-female" => ROLE_DIALOGUE_FEMALE,
        _ => ROLE_NARRATOR,
    }
}

/// Run the LLM extract pass against `chapter_body` and return the
/// segment list. Falls back to a single narrator-tagged segment
/// covering the whole prose when the LLM call or parsing fails — the
/// audio pipeline always has *something* to narrate, even if
/// multi-voice silently degrades to single-voice for that chapter.
pub async fn extract_segments(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    language: &str,
    chapter_title: &str,
    chapter_body: &str,
) -> Vec<VoiceSegment> {
    let body = chapter_body.trim();
    if body.is_empty() {
        return Vec::new();
    }

    let picked = match pick_llm_for_roles_lang(
        state,
        &[LlmRole::VoiceExtract, LlmRole::Chapter],
        Some(language),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "voice_extract: pick model failed; degrading to single-voice");
            return single_segment_fallback(chapter_body);
        }
    };

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("chapter_title", chapter_title.to_string());
    vars.insert("chapter_body", chapter_body.to_string());

    let rendered =
        match crate::generation::prompts::render(state, PromptRole::VoiceExtract, &vars).await {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "voice_extract: render prompt failed; degrading");
                return single_segment_fallback(chapter_body);
            }
        };

    let req = ChatRequest {
        model: picked.model_id.clone(),
        messages: vec![
            ChatMessage::system("Respond with one JSON object only."),
            ChatMessage::user(rendered.body),
        ],
        temperature: Some(0.2),
        // Chapter prose can be ~5k tokens; allow ample headroom for
        // the round-tripped JSON (which adds ~30% structural overhead).
        max_tokens: Some(8000),
        json_mode: Some(true),
        modalities: None,
        provider: Some(picked.provider.clone()),
    };

    let response = match state.llm().chat(&req).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "voice_extract: chat failed; degrading");
            log_generation_event(
                state,
                user,
                Some(audiobook_id),
                &picked.llm_id,
                PromptRole::VoiceExtract,
                &dummy_response(),
                Some(&e.to_string()),
            )
            .await
            .ok();
            return single_segment_fallback(chapter_body);
        }
    };

    if let Err(e) = log_generation_event(
        state,
        user,
        Some(audiobook_id),
        &picked.llm_id,
        PromptRole::VoiceExtract,
        &response,
        None,
    )
    .await
    {
        warn!(error = %e, "voice_extract: log generation event failed");
    }

    let cleaned = strip_code_fences(&response.content);
    let parsed: ExtractResponse = match serde_json::from_str(cleaned) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, raw = %cleaned, "voice_extract: parse failed; degrading");
            return single_segment_fallback(chapter_body);
        }
    };

    if parsed.segments.is_empty() {
        return single_segment_fallback(chapter_body);
    }

    parsed
        .segments
        .into_iter()
        .filter_map(|s| {
            let text = s.text;
            if text.trim().is_empty() {
                return None;
            }
            Some(VoiceSegment {
                role: canonical_role(&s.role).to_string(),
                text,
            })
        })
        .collect()
}

/// Persist segments onto the chapter row and return them. Keeps the
/// shape simple — `voice_segments` is `option<array<object>>` in the
/// schema (FLEXIBLE), so we round-trip through `serde_json::Value`.
pub async fn cache_segments(
    state: &AppState,
    chapter_raw_id: &str,
    segments: &[VoiceSegment],
) -> Result<()> {
    let value = serde_json::to_value(segments)
        .map_err(|e| Error::Other(anyhow::anyhow!("voice_segments json: {e}")))?;
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE chapter:`{chapter_raw_id}` SET voice_segments = $segs"
        ))
        .bind(("segs", value))
        .await
        .map_err(|e| Error::Database(format!("persist voice_segments: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("persist voice_segments: {e}")))?;
    Ok(())
}

/// Run the extract pass and cache the result. Returns the segments
/// the caller should narrate. Caller is responsible for checking
/// whether multi-voice is enabled on the audiobook.
pub async fn extract_and_cache(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    chapter_raw_id: &str,
    language: &str,
    chapter_title: &str,
    chapter_body: &str,
) -> Vec<VoiceSegment> {
    let segments = extract_segments(
        state,
        user,
        audiobook_id,
        language,
        chapter_title,
        chapter_body,
    )
    .await;
    if let Err(e) = cache_segments(state, chapter_raw_id, &segments).await {
        warn!(error = %e, chapter = chapter_raw_id, "voice_extract: cache failed");
    }
    segments
}

fn single_segment_fallback(body: &str) -> Vec<VoiceSegment> {
    vec![VoiceSegment {
        role: ROLE_NARRATOR.to_string(),
        text: body.to_string(),
    }]
}

fn strip_code_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

/// Build an empty ChatResponse for the failure-path log event so the
/// generation_event row still gets created with cost = 0 and the
/// error message attached. Mirrors the shape elsewhere in
/// `generation/`.
fn dummy_response() -> crate::llm::ChatResponse {
    crate::llm::ChatResponse {
        content: String::new(),
        image_base64: None,
        usage: crate::llm::ChatUsage::default(),
        mocked: false,
    }
}
