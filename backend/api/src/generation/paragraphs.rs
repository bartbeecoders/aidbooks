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

/// Per-paragraph diagram label produced by [`extract_visual_kinds`]
/// for STEM books. Carries the chosen template id and the
/// template-specific parameter blob (free-form JSON since each
/// template owns its own param schema).
#[derive(Debug, Clone)]
pub struct ParagraphVisual {
    pub visual_kind: String,
    pub visual_params: serde_json::Value,
}

/// Allowed `visual_kind` values. Mirrors the enumeration in
/// `paragraph_visual_v1.md`; the LLM can only pick from this list and
/// anything else is dropped silently. New templates added to G.4 must
/// extend this list and the prompt at the same time.
pub const ALLOWED_VISUAL_KINDS: &[&str] = &[
    "function_plot",
    "axes_with_curve",
    "vector_field",
    "free_body",
    "flow_chart",
    "bar_chart",
    "equation_steps",
    "neural_net_layer",
    // Phase H — escape hatch for paragraphs no template fits well.
    // The classifier picks this when none of the structured templates
    // would do justice; the per-paragraph code-gen LLM then writes a
    // bespoke Manim Scene class. Persisted as `manim_code` on the
    // paragraph object alongside `visual_kind`.
    "custom_manim",
];

/// Hard cap on diagram labels per chapter. Keeps the Manim render
/// budget bounded even if the LLM marks every paragraph visual.
const MAX_VISUAL_DIAGRAMS_PER_CHAPTER: usize = 8;

#[derive(Debug, Deserialize)]
struct VisualResponse {
    #[serde(default)]
    visuals: Vec<VisualEntry>,
}

#[derive(Debug, Deserialize)]
struct VisualEntry {
    index: u32,
    #[serde(default)]
    visual_kind: String,
    #[serde(default)]
    visual_params: serde_json::Value,
}

/// Per-paragraph diagram classifier. STEM-only — caller gates on
/// `audiobook.is_stem` before invoking. Returns a map of
/// `paragraph_index -> ParagraphVisual` for paragraphs the LLM
/// considers diagrammatic; anything else stays prose.
///
/// Same degrade-gracefully posture as [`extract_scenes`]: parse /
/// network failures log a warn and return an empty map so the
/// orchestrator can still persist the paragraph list (the chapter
/// just renders without diagrams).
#[allow(clippy::too_many_arguments)]
pub async fn extract_visual_kinds(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    book_title: &str,
    book_topic: &str,
    genre: Option<&str>,
    chapter_title: &str,
    paragraphs: &[Paragraph],
) -> std::collections::HashMap<u32, ParagraphVisual> {
    let mut out = std::collections::HashMap::new();
    if paragraphs.is_empty() {
        return out;
    }

    let listing: String = paragraphs
        .iter()
        .map(|p| {
            let excerpt: String = p.text.chars().take(600).collect();
            format!("[{}] {}\n", p.index, excerpt)
        })
        .collect();

    let mut vars: std::collections::HashMap<&str, String> =
        std::collections::HashMap::new();
    vars.insert("book_title", book_title.to_string());
    vars.insert("book_topic", book_topic.to_string());
    vars.insert("genre", genre.unwrap_or("any").to_string());
    vars.insert("chapter_title", chapter_title.to_string());
    vars.insert("paragraph_listing", listing);

    let rendered = match crate::generation::prompts::render(
        state,
        PromptRole::ParagraphVisual,
        &vars,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "paragraph visual extract: load template failed");
            return out;
        }
    };

    // Prefer the user's configured Manim LLM (`LlmRole::ManimCode`) so
    // the same model handles both the diagram classifier and the
    // bespoke code-gen — admins who pin a coder model (DeepSeek-Coder,
    // Qwen-Coder) for Manim get it driving the labelling step too,
    // which keeps both halves of the diagram pipeline coherent. Falls
    // back to the prose `Chapter` model when no row is tagged for
    // `ManimCode`, so existing setups keep working unchanged.
    let picked =
        match crate::llm::pick_llm_for_roles_lang(state, &[LlmRole::ManimCode, LlmRole::Chapter], None)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "paragraph visual extract: pick model failed");
                return out;
            }
        };

    let req = ChatRequest {
        model: picked.model_id.clone(),
        messages: vec![
            ChatMessage::system(
                "You label STEM paragraphs with diagram templates. \
                 Reply with strict JSON only — no prose, no markdown.",
            ),
            ChatMessage::user(rendered.body),
        ],
        // Lower temp than scene extract — we want stable labels, not
        // creative descriptions.
        temperature: Some(0.2),
        max_tokens: Some(2_000),
        json_mode: Some(true),
        modalities: None,
        provider: Some(picked.provider.clone()),
    };

    let resp = match state.llm().chat(&req).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "paragraph visual extract: chat failed");
            log_generation_event(
                state,
                user,
                Some(audiobook_id),
                &picked.llm_id,
                PromptRole::ParagraphVisual,
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
        PromptRole::ParagraphVisual,
        &resp,
        None,
    )
    .await
    .ok();

    let content = strip_code_fences(&resp.content);
    let parsed: VisualResponse = match serde_json::from_str::<VisualResponse>(content) {
        Ok(p) => p,
        Err(_) => {
            // Some models emit the bare `[...]` array.
            match serde_json::from_str::<Vec<VisualEntry>>(content) {
                Ok(v) => VisualResponse { visuals: v },
                Err(e) => {
                    warn!(
                        error = %e,
                        preview = %content.chars().take(300).collect::<String>(),
                        "paragraph visual extract: JSON parse failed"
                    );
                    return out;
                }
            }
        }
    };

    let valid_indices: std::collections::HashSet<u32> =
        paragraphs.iter().map(|p| p.index).collect();

    for entry in parsed.visuals {
        if !valid_indices.contains(&entry.index) {
            // LLM hallucinated an index that wasn't in the listing.
            continue;
        }
        let kind = entry.visual_kind.trim();
        if !ALLOWED_VISUAL_KINDS.contains(&kind) {
            // Unknown kind — drop it silently rather than fail the
            // whole pass on one bad row.
            continue;
        }
        if entry.visual_params.is_null() {
            // Allow but normalize to empty object so consumers don't
            // have to special-case null.
            out.insert(
                entry.index,
                ParagraphVisual {
                    visual_kind: kind.to_string(),
                    visual_params: serde_json::json!({}),
                },
            );
        } else if entry.visual_params.is_object() {
            out.insert(
                entry.index,
                ParagraphVisual {
                    visual_kind: kind.to_string(),
                    visual_params: entry.visual_params,
                },
            );
        }
        // visual_params not an object → drop this entry; the template
        // requires a structured shape.
    }

    // Cap diagrams per chapter, keeping the lowest-indexed (earliest
    // in reading order) so the chapter's diagram timeline stays
    // coherent.
    if out.len() > MAX_VISUAL_DIAGRAMS_PER_CHAPTER {
        let mut keys: Vec<u32> = out.keys().copied().collect();
        keys.sort_unstable();
        for drop_key in keys.into_iter().skip(MAX_VISUAL_DIAGRAMS_PER_CHAPTER) {
            out.remove(&drop_key);
        }
    }

    out
}

/// Merge the splitter output with the extract maps into the JSON shape
/// stored on `chapter.paragraphs`. Each entry carries `index`, `text`,
/// `char_count`, optional `scene_description`, an empty `image_paths`
/// array (image jobs fill it in as they complete), and — for STEM
/// books that ran the diagram classifier — optional `visual_kind` +
/// `visual_params`.
///
/// `visuals` is empty for non-STEM books and for STEM books whose
/// classifier pass didn't tag any paragraphs. Either way the field is
/// omitted from the persisted JSON when there's nothing to record, so
/// the SCHEMAFULL row stays slim.
pub fn merge_for_persist(
    paragraphs: &[Paragraph],
    scenes: &std::collections::HashMap<u32, String>,
    visuals: &std::collections::HashMap<u32, ParagraphVisual>,
    manim_codes: &std::collections::HashMap<u32, String>,
) -> Vec<serde_json::Value> {
    paragraphs
        .iter()
        .map(|p| {
            let scene = scenes.get(&p.index).cloned();
            let visual = visuals.get(&p.index);
            let mut entry = serde_json::Map::new();
            entry.insert("index".into(), json!(p.index));
            entry.insert("text".into(), json!(p.text));
            entry.insert("char_count".into(), json!(p.char_count));
            entry.insert("scene_description".into(), json!(scene));
            entry.insert("image_paths".into(), json!(Vec::<String>::new()));
            if let Some(v) = visual {
                entry.insert("visual_kind".into(), json!(v.visual_kind));
                entry.insert("visual_params".into(), v.visual_params.clone());
            }
            // Phase H — only emit `manim_code` when the classifier
            // also tagged this paragraph as `custom_manim`. Empty
            // strings are dropped on the floor: the publisher treats
            // missing == empty == fall-back-to-prose.
            if let Some(code) = manim_codes.get(&p.index) {
                if !code.trim().is_empty() {
                    entry.insert("manim_code".into(), json!(code));
                }
            }
            serde_json::Value::Object(entry)
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
        let visuals = std::collections::HashMap::new();
        let codes = std::collections::HashMap::new();
        let out = merge_for_persist(&paras, &scenes, &visuals, &codes);
        assert_eq!(out[0]["scene_description"], "a hill at dawn");
        assert!(out[1]["scene_description"].is_null());
        // No diagrams labelled → visual_kind must be absent (not null).
        assert!(out[0].get("visual_kind").is_none());
        assert!(out[1].get("visual_kind").is_none());
    }

    #[test]
    fn merge_includes_visual_kind_when_labelled() {
        let p = "a".repeat(MIN_PARAGRAPH_CHARS);
        let paras = split(&format!("{p}\n\n{p}"));
        let scenes = std::collections::HashMap::new();
        let mut visuals = std::collections::HashMap::new();
        visuals.insert(
            0u32,
            ParagraphVisual {
                visual_kind: "free_body".into(),
                visual_params: serde_json::json!({
                    "object": "block on incline",
                    "forces": ["gravity", "normal"]
                }),
            },
        );
        let codes = std::collections::HashMap::new();
        let out = merge_for_persist(&paras, &scenes, &visuals, &codes);
        assert_eq!(out[0]["visual_kind"], "free_body");
        assert_eq!(
            out[0]["visual_params"]["object"],
            "block on incline"
        );
        // Unlabelled paragraph stays diagram-free.
        assert!(out[1].get("visual_kind").is_none());
    }

    #[test]
    fn merge_includes_manim_code_when_present() {
        let p = "a".repeat(MIN_PARAGRAPH_CHARS);
        let paras = split(&format!("{p}\n\n{p}"));
        let scenes = std::collections::HashMap::new();
        let mut visuals = std::collections::HashMap::new();
        visuals.insert(
            0u32,
            ParagraphVisual {
                visual_kind: "custom_manim".into(),
                visual_params: serde_json::json!({}),
            },
        );
        let mut codes = std::collections::HashMap::new();
        codes.insert(0u32, "class Scene(TemplateScene): pass".to_string());
        // Empty/whitespace code should NOT round-trip through merge.
        codes.insert(1u32, "   ".to_string());
        let out = merge_for_persist(&paras, &scenes, &visuals, &codes);
        assert_eq!(out[0]["visual_kind"], "custom_manim");
        assert_eq!(
            out[0]["manim_code"],
            "class Scene(TemplateScene): pass"
        );
        assert!(out[1].get("manim_code").is_none());
    }

    #[test]
    fn allowed_visual_kinds_is_non_empty() {
        // Cheap sanity check: if the const ever drifts to empty,
        // every diagram label silently drops in extract_visual_kinds.
        assert!(!ALLOWED_VISUAL_KINDS.is_empty());
        assert!(ALLOWED_VISUAL_KINDS.contains(&"function_plot"));
        assert!(ALLOWED_VISUAL_KINDS.contains(&"equation_steps"));
    }
}
