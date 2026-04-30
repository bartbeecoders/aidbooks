//! Paragraph splitter + scene-extract LLM pass.
//!
//! Pipeline:
//!   1. `split` markdown chapter body on blank lines → ordered list of
//!      raw paragraph strings, filtered to drop very short blocks
//!      (dialogue tags, attribution lines, etc.).
//!   2. `extract_scenes` calls a JSON-mode LLM with the numbered
//!      paragraph list and asks which paragraphs are visualizable +
//!      what the scene description should be. Non-visual paragraphs
//!      are simply absent from the response.
//!   3. `merge` zips the splitter output and the LLM scene list into
//!      the persistable shape (one entry per paragraph, with optional
//!      `scene_description` and an empty `image_paths` array).
//!
//! The persisted shape lives on `chapter.paragraphs` (FLEXIBLE
//! `array<object>`); the orchestrator job reads it back to fan out
//! per-paragraph image jobs.

use listenai_core::domain::{LlmRole, PromptRole};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

use crate::generation::outline::log_generation_event;
use crate::llm::{pick_llm_for_role, ChatMessage, ChatRequest};
use crate::state::AppState;

/// Minimum char length for a paragraph to be a candidate. Below this we
/// assume it's a heading, dialogue tag, or attribution line that would
/// produce a weak image regardless.
const MIN_PARAGRAPH_CHARS: usize = 80;

/// Hard cap on the number of paragraphs we send to the extract pass.
/// Keeps the prompt budget bounded for very long chapters; extras are
/// silently dropped (most chapters fit comfortably under this).
const MAX_PARAGRAPHS_PER_CHAPTER: usize = 30;

/// Hard cap on the number of *visual* paragraphs we render per chapter.
/// Combined with the per-book `images_per_paragraph` cap, this bounds
/// total image cost for an oddly-structured chapter where the LLM marks
/// nearly everything visual.
pub const MAX_VISUAL_PARAGRAPHS_PER_CHAPTER: usize = 12;

#[derive(Debug, Clone)]
pub struct Paragraph {
    pub index: u32,
    pub text: String,
    pub char_count: u32,
}

/// Markdown-blank-line split with a min-length filter. Indices are
/// assigned to the *kept* paragraphs in body order, so they're stable
/// across image-gen runs as long as the chapter body doesn't change.
pub fn split(body_md: &str) -> Vec<Paragraph> {
    body_md
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| p.chars().count() >= MIN_PARAGRAPH_CHARS)
        .take(MAX_PARAGRAPHS_PER_CHAPTER)
        .enumerate()
        .map(|(i, text)| Paragraph {
            index: i as u32,
            char_count: text.chars().count() as u32,
            text: text.to_string(),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct ExtractResponse {
    /// Some models wrap their output in `{"scenes": [...]}`. Accept both
    /// that and a top-level array via the `from` Vec path below.
    #[serde(default)]
    scenes: Vec<ExtractScene>,
}

#[derive(Debug, Deserialize)]
struct ExtractScene {
    index: u32,
    #[serde(default)]
    scene: String,
}

/// Call the LLM with the paragraph list + a JSON-only system prompt.
/// Returns a map of `paragraph_index -> scene_description` for every
/// paragraph the model considers visualizable. Non-visual paragraphs
/// are absent from the map.
///
/// On parse / network failures we degrade gracefully: log a warn and
/// return an empty map so the orchestrator can still persist the
/// paragraph list (image jobs simply won't fan out).
#[allow(clippy::too_many_arguments)]
pub async fn extract_scenes(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    book_title: &str,
    book_topic: &str,
    genre: Option<&str>,
    chapter_title: &str,
    paragraphs: &[Paragraph],
) -> std::collections::HashMap<u32, String> {
    let mut out = std::collections::HashMap::new();
    if paragraphs.is_empty() {
        return out;
    }

    // Numbered list — match the indexes the splitter assigned so the
    // model's response is unambiguous.
    let listing: String = paragraphs
        .iter()
        .map(|p| {
            let excerpt: String = p.text.chars().take(600).collect();
            format!("[{}] {}\n", p.index, excerpt)
        })
        .collect();

    let user_msg = format!(
        "Book title: {book_title}\nBook topic: {book_topic}\nGenre: {genre}\n\
         Chapter: {chapter_title}\n\n\
         Paragraphs:\n{listing}\n\
         Pick the paragraphs that genuinely have visual content — concrete \
         settings, characters, action, or evocative imagery. Skip \
         dialogue-only blocks, abstract reflection, and exposition with no \
         imagery.\n\n\
         For each picked paragraph, write a vivid one-sentence scene \
         description suitable for a text-to-image model: focus on subject, \
         setting, mood, and lighting. Do NOT mention text, captions, or \
         numbers — the image model should not draw any.\n\n\
         Return STRICT JSON of the form:\n\
         {{\"scenes\": [{{\"index\": <int>, \"scene\": \"...\"}}, ...]}}\n\
         Indices must match the bracketed numbers above. Do not invent \
         indices that aren't listed.",
        genre = genre.unwrap_or("any"),
    );

    let picked = match pick_llm_for_role(state, LlmRole::Chapter).await {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "paragraph scene extract: pick model failed");
            return out;
        }
    };

    let req = ChatRequest {
        model: picked.model_id.clone(),
        messages: vec![
            ChatMessage::system(
                "You identify visual moments in audiobook chapters. \
                 Reply with strict JSON only — no prose, no markdown.",
            ),
            ChatMessage::user(user_msg),
        ],
        temperature: Some(0.4),
        max_tokens: Some(2_000),
        json_mode: Some(true),
        modalities: None,
        provider: Some(picked.provider.clone()),
    };

    let resp = match state.llm().chat(&req).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "paragraph scene extract: chat failed");
            log_generation_event(
                state,
                user,
                Some(audiobook_id),
                &picked.llm_id,
                PromptRole::SceneExtract,
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
            return out;
        }
    };
    log_generation_event(
        state,
        user,
        Some(audiobook_id),
        &picked.llm_id,
        PromptRole::SceneExtract,
        &resp,
        None,
    )
    .await
    .ok();
    let content = strip_code_fences(&resp.content);
    let parsed: ExtractResponse =
        match serde_json::from_str::<ExtractResponse>(content) {
            Ok(p) => p,
            Err(_) => {
                // Some models return a bare top-level array. Try that
                // before giving up.
                match serde_json::from_str::<Vec<ExtractScene>>(content) {
                    Ok(scenes) => ExtractResponse { scenes },
                    Err(e) => {
                        warn!(
                            error = %e,
                            preview = %content.chars().take(300).collect::<String>(),
                            "paragraph scene extract: JSON parse failed"
                        );
                        return out;
                    }
                }
            }
        };

    for s in parsed.scenes {
        let scene = s.scene.trim();
        if scene.is_empty() {
            continue;
        }
        out.insert(s.index, scene.to_string());
    }

    // Cap visual paragraphs per chapter. We keep the lowest-indexed
    // ones (i.e. earlier in reading order) so the slideshow timeline
    // stays coherent rather than dropping random middle paragraphs.
    if out.len() > MAX_VISUAL_PARAGRAPHS_PER_CHAPTER {
        let mut keys: Vec<u32> = out.keys().copied().collect();
        keys.sort_unstable();
        for drop_key in keys.into_iter().skip(MAX_VISUAL_PARAGRAPHS_PER_CHAPTER) {
            out.remove(&drop_key);
        }
    }

    out
}

/// Merge the splitter output with the extract map into the JSON shape
/// stored on `chapter.paragraphs`. Each entry carries `index`, `text`,
/// `char_count`, optional `scene_description`, and an empty
/// `image_paths` array (image jobs fill it in as they complete).
pub fn merge_for_persist(
    paragraphs: &[Paragraph],
    scenes: &std::collections::HashMap<u32, String>,
) -> Vec<serde_json::Value> {
    paragraphs
        .iter()
        .map(|p| {
            let scene = scenes.get(&p.index).cloned();
            json!({
                "index": p.index,
                "text": p.text,
                "char_count": p.char_count,
                "scene_description": scene,
                "image_paths": Vec::<String>::new(),
            })
        })
        .collect()
}

/// Persist the merged paragraph list onto a chapter row. Replaces
/// whatever was there before — orchestrator runs are full rewrites.
pub async fn persist(
    state: &AppState,
    chapter_id: &str,
    paragraphs: Vec<serde_json::Value>,
) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE chapter:`{chapter_id}` SET paragraphs = $p"
        ))
        .bind(("p", paragraphs))
        .await
        .map_err(|e| Error::Database(format!("persist paragraphs: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("persist paragraphs: {e}")))?;
    Ok(())
}

fn strip_code_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_drops_short_blocks() {
        let body = "Tiny.\n\nThis paragraph is comfortably long enough to clear the \
                    minimum-length filter so it should be kept by the splitter and \
                    given an index of zero.\n\nAlso short.";
        let out = split(body);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].index, 0);
    }

    #[test]
    fn split_assigns_indices_to_kept_paragraphs() {
        let p = "a".repeat(MIN_PARAGRAPH_CHARS);
        let body = format!("{p}\n\nshort\n\n{p}");
        let out = split(&body);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].index, 0);
        assert_eq!(out[1].index, 1);
    }

    #[test]
    fn merge_marks_visual_paragraphs() {
        let p = "a".repeat(MIN_PARAGRAPH_CHARS);
        let paras = split(&format!("{p}\n\n{p}"));
        let mut scenes = std::collections::HashMap::new();
        scenes.insert(0u32, "a hill at dawn".to_string());
        let out = merge_for_persist(&paras, &scenes);
        assert_eq!(out[0]["scene_description"], "a hill at dawn");
        assert!(out[1]["scene_description"].is_null());
    }
}
