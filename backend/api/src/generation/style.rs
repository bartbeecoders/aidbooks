//! Render the per-book narration style overlay that the outline and
//! chapter prompts inject as their `{{style_overlay}}` variable.
//!
//! Two inputs combine:
//!
//!   * `NarrationStyle` — single style hint that reshapes plot + tone
//!     (e.g. `Drama`, `Humor`, `ChildFriendly`). One-line guidance per
//!     style lives on the enum itself (`prompt_hint`).
//!   * `NarrationIntensity` — additive emotional dial. Multiple tags
//!     stack: `[Intense, Dramatic]` is louder than `[Intense]` alone.
//!
//! The block we produce always ends with a short "delivery" line so the
//! chapter writer knows to lean on the speech-tag palette (`<whisper>`,
//! `<loud>`, `[pause]`, …) when an intensity tag asks for it. Empty when
//! both inputs are empty/`Natural` — the outline prompt then sees the
//! string `"(no style overlay — write in the genre's natural register)"`
//! so it never gets a dangling colon.

use listenai_core::domain::{NarrationIntensity, NarrationStyle};

/// Build the multi-line hint the prompt embeds verbatim. Caller picks
/// up the result and passes it as the `style_overlay` template var.
pub fn render_style_overlay(
    style: Option<NarrationStyle>,
    intensity: &[NarrationIntensity],
) -> String {
    let style_line = style.and_then(|s| {
        let hint = s.prompt_hint();
        if hint.is_empty() {
            None
        } else {
            Some(format!("- Style: {hint}"))
        }
    });

    let intensity_line = if intensity.is_empty() {
        None
    } else {
        let tags: Vec<&str> = intensity.iter().map(|i| i.as_str()).collect();
        Some(format!(
            "- Emotional dial: {}. Use the speech-tag palette (e.g. `<whisper>`, `<loud>`, `<fast>`, `[pause]`) to embody these in delivery — don't just describe emotion, perform it through tag placement and sentence rhythm.",
            tags.join(" + ")
        ))
    };

    let mut lines: Vec<String> = Vec::new();
    if let Some(s) = style_line {
        lines.push(s);
    }
    if let Some(i) = intensity_line {
        lines.push(i);
    }
    if lines.is_empty() {
        "(no style overlay — write in the genre's natural register)".to_string()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inputs_render_neutral_placeholder() {
        let s = render_style_overlay(None, &[]);
        assert!(s.contains("no style overlay"));
        let s = render_style_overlay(Some(NarrationStyle::Natural), &[]);
        assert!(s.contains("no style overlay"));
    }

    #[test]
    fn style_only_renders_single_line() {
        let s = render_style_overlay(Some(NarrationStyle::Drama), &[]);
        assert!(s.starts_with("- Style:"));
        assert!(!s.contains("Emotional dial"));
    }

    #[test]
    fn intensity_only_renders_dial_line() {
        let s = render_style_overlay(
            None,
            &[NarrationIntensity::Intense, NarrationIntensity::Dramatic],
        );
        assert!(s.contains("Emotional dial: intense + dramatic"));
        assert!(!s.contains("Style:"));
    }

    #[test]
    fn both_render_two_lines_with_speech_tag_hint() {
        let s = render_style_overlay(
            Some(NarrationStyle::Humor),
            &[NarrationIntensity::Expressive],
        );
        assert!(s.contains("Style:"));
        assert!(s.contains("Emotional dial: expressive"));
        // The speech-tag bridge is what lets intensity influence TTS,
        // not just LLM word choice — make sure it survives.
        assert!(s.contains("speech-tag palette"));
    }
}
