//! Phase H — bespoke Manim code generator.
//!
//! Runs after [`extract_visual_kinds`] when the classifier marks one or
//! more paragraphs as `visual_kind = "custom_manim"`. For each such
//! paragraph we render the `manim_code_v1` prompt against the
//! [`LlmRole::ManimCode`] model (which the user can point at a
//! code-specialised LLM independent of the prose model) and capture
//! the returned Scene class body.
//!
//! The result is keyed by paragraph index, mirroring how
//! [`extract_visual_kinds`] returns its `HashMap<u32, ParagraphVisual>`.
//! `merge_for_persist` already understands `manim_code` because we
//! widened it in this phase; the publisher's `load_paragraph_tiles`
//! reads it back when assembling the SceneSpec.

use std::collections::HashMap;

use listenai_core::domain::{LlmRole, PromptRole};
use listenai_core::id::UserId;
use tracing::{debug, warn};

use crate::generation::outline::log_generation_event;
use crate::generation::paragraphs::Paragraph;
use crate::llm::{pick_llm_for_role, ChatMessage, ChatRequest};
use crate::state::AppState;

/// Output of the code-gen pass for one paragraph.
#[derive(Debug, Clone)]
pub struct ParagraphCode {
    /// One-sentence description of what the scene shows. Persisted
    /// for diagnostics; not used by the renderer. Empty string when
    /// the LLM declined (returned `code: ""`).
    #[allow(dead_code)]
    pub summary: String,
    /// Python source for a `class Scene(TemplateScene): ...` block.
    /// Empty when the LLM decided the paragraph isn't visualisable;
    /// the caller treats empty as "fall back to prose".
    pub code: String,
}

#[derive(Debug, serde::Deserialize)]
struct CodeResponse {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    code: String,
}

/// Generate Manim code for every paragraph the classifier marked
/// `visual_kind = "custom_manim"`. Returns a map keyed by paragraph
/// index. Paragraphs with empty/whitespace-only generated code are
/// included with `code = ""`; the publisher treats those as
/// fall-back-to-prose so the chapter still renders.
///
/// Theme/run_seconds are passed into the prompt so the LLM can pace
/// `self.play(...)` against the actual budget. `paragraphs_visual`
/// is the list of paragraphs the classifier picked as custom_manim,
/// pruned by the caller — anything else is wasted token budget.
#[allow(clippy::too_many_arguments)]
pub async fn generate_manim_code(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    book_title: &str,
    book_topic: &str,
    genre: Option<&str>,
    chapter_title: &str,
    theme: &str,
    custom_paragraphs: &[CustomParagraph<'_>],
) -> HashMap<u32, ParagraphCode> {
    let mut out: HashMap<u32, ParagraphCode> = HashMap::new();
    if custom_paragraphs.is_empty() {
        return out;
    }

    let picked = match pick_llm_for_role(state, LlmRole::ManimCode).await {
        Ok(p) => p,
        Err(e) => {
            // Fall back to the prose model. Same rationale as the
            // ParagraphVisual path: we'd rather degrade than hard-fail
            // the whole render — the AST screen on the sidecar still
            // catches anything dangerous a non-coding model emits.
            warn!(error = %e, "manim_code: pick model failed; falling back to chapter role");
            match pick_llm_for_role(state, LlmRole::Chapter).await {
                Ok(p) => p,
                Err(e2) => {
                    warn!(error = %e2, "manim_code: fallback chapter pick also failed; skipping");
                    return out;
                }
            }
        }
    };

    for cp in custom_paragraphs {
        let mut vars: HashMap<&str, String> = HashMap::new();
        vars.insert("book_title", book_title.to_string());
        vars.insert("book_topic", book_topic.to_string());
        vars.insert("genre", genre.unwrap_or("any").to_string());
        vars.insert("chapter_title", chapter_title.to_string());
        vars.insert("theme", theme.to_string());
        vars.insert("run_seconds", format!("{:.1}", cp.run_seconds));
        vars.insert("paragraph_text", cp.text.to_string());

        let rendered = match crate::generation::prompts::render(
            state,
            PromptRole::ManimCode,
            &vars,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, idx = cp.index, "manim_code: render prompt failed");
                continue;
            }
        };

        let req = ChatRequest {
            model: picked.model_id.clone(),
            messages: vec![
                ChatMessage::system(
                    "You write Manim Community Edition code for one diagram. \
                     Reply with strict JSON: {\"summary\": \"...\", \"code\": \"...\"}. \
                     No markdown fences, no prose outside the JSON.",
                ),
                ChatMessage::user(rendered.body),
            ],
            // Higher than visual classifier (0.2) but lower than chapter
            // generation — code-gen wants some creative latitude on the
            // visual choice but should be deterministic in syntax.
            temperature: Some(0.4),
            // Generous: a real Scene class is 30–80 lines + the JSON
            // wrapping overhead. Keep headroom so we don't truncate.
            max_tokens: Some(4_000),
            json_mode: Some(true),
            modalities: None,
            provider: Some(picked.provider.clone()),
        };

        let resp = match state.llm().chat(&req).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, idx = cp.index, "manim_code: chat failed");
                log_generation_event(
                    state,
                    user,
                    Some(audiobook_id),
                    &picked.llm_id,
                    PromptRole::ManimCode,
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
                continue;
            }
        };
        log_generation_event(
            state,
            user,
            Some(audiobook_id),
            &picked.llm_id,
            PromptRole::ManimCode,
            &resp,
            None,
        )
        .await
        .ok();

        let content = strip_code_fences(&resp.content);
        let parsed: CodeResponse = match serde_json::from_str(content) {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    error = %e,
                    idx = cp.index,
                    preview = %content.chars().take(200).collect::<String>(),
                    "manim_code: JSON parse failed"
                );
                continue;
            }
        };

        // Don't persist cosmetic whitespace.
        let code = parsed.code.trim().to_string();
        let summary = parsed.summary.trim().to_string();

        debug!(
            idx = cp.index,
            code_bytes = code.len(),
            summary = %summary,
            "manim_code: generated"
        );
        out.insert(
            cp.index,
            ParagraphCode { summary, code },
        );
    }

    out
}

/// Slim view of a paragraph the caller wants code generated for.
/// Borrowed instead of owned so the caller can fan out from
/// `&[Paragraph]` without cloning bodies.
#[derive(Debug, Clone, Copy)]
pub struct CustomParagraph<'a> {
    pub index: u32,
    pub text: &'a str,
    /// Allotted runtime in seconds. The publisher computes this from
    /// the paragraph's audio window so the Manim animation paces
    /// against the actual duration the user will see.
    pub run_seconds: f32,
}

/// Build a list of [`CustomParagraph`]s from the classifier output.
/// Filters paragraphs to those marked `custom_manim`. `run_seconds`
/// is taken from the paragraph audio plan when available and
/// defaults to 6.0 s per paragraph otherwise (matching the
/// MIN_RUN_SECONDS floor on the Python side at 2.0 s, with extra
/// headroom for a paragraph-length scene).
pub fn custom_paragraphs<'a>(
    paragraphs: &'a [Paragraph],
    visual_kinds: &HashMap<u32, String>,
    durations_ms: &HashMap<u32, u64>,
) -> Vec<CustomParagraph<'a>> {
    paragraphs
        .iter()
        .filter_map(|p| {
            let kind = visual_kinds.get(&p.index)?;
            if kind != "custom_manim" {
                return None;
            }
            let dur_ms = durations_ms.get(&p.index).copied().unwrap_or(6_000);
            Some(CustomParagraph {
                index: p.index,
                text: &p.text,
                run_seconds: (dur_ms as f32 / 1000.0).max(2.0),
            })
        })
        .collect()
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
    fn custom_paragraphs_filters_by_kind() {
        let paragraphs = vec![
            Paragraph {
                index: 0,
                text: "Para zero".into(),
                char_count: 9,
            },
            Paragraph {
                index: 1,
                text: "Para one".into(),
                char_count: 8,
            },
        ];
        let mut kinds = HashMap::new();
        kinds.insert(0u32, "function_plot".to_string());
        kinds.insert(1u32, "custom_manim".to_string());
        let mut durations = HashMap::new();
        durations.insert(1u32, 12_000);

        let out = custom_paragraphs(&paragraphs, &kinds, &durations);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].index, 1);
        assert!((out[0].run_seconds - 12.0).abs() < 0.01);
    }

    #[test]
    fn custom_paragraphs_floors_run_seconds() {
        let paragraphs = vec![Paragraph {
            index: 5,
            text: "tiny".into(),
            char_count: 4,
        }];
        let mut kinds = HashMap::new();
        kinds.insert(5u32, "custom_manim".to_string());
        let durations = HashMap::new(); // missing → defaults to 6_000ms

        let out = custom_paragraphs(&paragraphs, &kinds, &durations);
        assert_eq!(out.len(), 1);
        assert!(out[0].run_seconds >= 2.0);
    }
}
