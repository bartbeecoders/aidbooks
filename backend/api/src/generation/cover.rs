//! Cover-art generation via OpenRouter.
//!
//! Uses an image-capable LLM (e.g. `google/gemini-2.5-flash-image`) selected
//! by the `cover_art` role. The model returns a base64-encoded PNG; callers
//! receive the decoded bytes.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use listenai_core::domain::LlmRole;
use listenai_core::{Error, Result};

use crate::llm::{pick_model_for_role, ChatMessage, ChatRequest};
use crate::state::AppState;

const SYSTEM: &str = "You generate audiobook cover artwork. Produce a single \
square cover image. No text, no captions, no chapter numbers — purely \
illustrative. Style: cinematic, evocative of the genre.";

/// Generate a cover image for the given topic + optional genre.
/// Returns the raw image bytes (typically PNG).
pub async fn generate(state: &AppState, topic: &str, genre: Option<&str>) -> Result<Vec<u8>> {
    let topic = topic.trim();
    if topic.is_empty() {
        return Err(Error::Validation("topic must not be empty".into()));
    }

    let model = pick_model_for_role(state, LlmRole::CoverArt).await?;
    let prompt = build_prompt(topic, genre);

    let req = ChatRequest {
        model,
        messages: vec![
            ChatMessage::system(SYSTEM),
            ChatMessage::user(prompt),
        ],
        temperature: Some(1.0),
        max_tokens: Some(1024),
        json_mode: None,
        modalities: Some(vec!["image".into(), "text".into()]),
    };

    let resp = state.llm().chat(&req).await?;
    let b64 = resp.image_base64.ok_or_else(|| {
        Error::Upstream(
            "openrouter: model did not return an image — check that the \
             model selected for `cover_art` supports image output"
                .into(),
        )
    })?;

    let bytes = B64
        .decode(b64.as_bytes())
        .map_err(|e| Error::Upstream(format!("decode cover base64: {e}")))?;
    if bytes.is_empty() {
        return Err(Error::Upstream("openrouter: empty image payload".into()));
    }
    Ok(bytes)
}

fn build_prompt(topic: &str, genre: Option<&str>) -> String {
    match genre.map(str::trim).filter(|g| !g.is_empty()) {
        Some(g) => format!(
            "Audiobook cover artwork.\nTopic: {topic}\nGenre: {g}\n\
             Compose a striking, atmospheric image that captures the mood. \
             Square format. No lettering of any kind."
        ),
        None => format!(
            "Audiobook cover artwork.\nTopic: {topic}\n\
             Compose a striking, atmospheric image that captures the mood. \
             Square format. No lettering of any kind."
        ),
    }
}
