//! Shared text + timing helpers used by both the SRT subtitle generator
//! (`youtube/subtitles.rs`) and the animation scene planner. Keeping them
//! in one place avoids the kind of subtle drift where the captions split
//! sentences one way and the karaoke text-reveal splits them another.
//!
//! Three concerns live here:
//!
//!   * [`strip_markdown`] — cheap markdown remover (headings, emphasis,
//!     links, fenced code) for cue / scene display. Not a parser.
//!   * [`split_sentences`] — sentence-terminator-driven splitter, keeps
//!     punctuation attached. Latin + CJK terminators.
//!   * [`split_paragraphs`] — animation-flavoured paragraph splitter:
//!     blank-line separated, all non-empty blocks kept, short blocks
//!     (< [`PARAGRAPH_MERGE_THRESHOLD`]) merged into the preceding
//!     paragraph so the animation timeline stays continuous.
//!     `generation::paragraphs::split` is the image-gen-flavoured cousin
//!     — it *drops* short blocks because they make weak art. Different
//!     callers, different invariants; on purpose.
//!   * [`ratio_to_ms`] — char-position → time mapping for char-rate
//!     proportional allocation. The TTS provider doesn't return word
//!     timing, so prose-rate variance is the best signal we have.

/// Below this many characters, a paragraph block is treated as
/// transitional (single-line dialogue, attribution) and merged onto the
/// preceding paragraph. Picked to match
/// `generation::paragraphs::split::MIN_PARAGRAPH_CHARS` so the two
/// splitters agree on what "real" paragraphs are even though they disagree
/// on what to do with the small ones.
pub const PARAGRAPH_MERGE_THRESHOLD: usize = 80;

/// Cheap markdown remover. Not a parser — just enough to clean up the
/// LLM's prose for caption/scene display:
///   * `# heading` → `heading`
///   * `**bold**`, `*italic*`, `_emph_` → unwrap
///   * `[text](url)` → `text`
///   * fenced code blocks → drop entirely (audio narration would have
///     skipped them anyway).
pub fn strip_markdown(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_code_fence = false;
    for raw_line in input.lines() {
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }
        let line = strip_line(trimmed);
        let line = line.trim();
        if line.is_empty() {
            // Preserve paragraph breaks as a single space — sentence splitter
            // doesn't need explicit blank lines.
            out.push(' ');
            continue;
        }
        out.push_str(line);
        out.push(' ');
    }
    // Collapse repeated whitespace.
    let mut prev_ws = false;
    let mut compact = String::with_capacity(out.len());
    for ch in out.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                compact.push(' ');
                prev_ws = true;
            }
        } else {
            compact.push(ch);
            prev_ws = false;
        }
    }
    compact.trim().to_string()
}

fn strip_line(line: &str) -> String {
    let line = line.trim_start_matches('#').trim_start();
    let line = line.trim_start_matches(['>', '-', '*'].as_slice());
    let line = line.trim_start();

    // [text](url) → text
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' => {
                let mut text = String::new();
                let mut closed = false;
                for nc in chars.by_ref() {
                    if nc == ']' {
                        closed = true;
                        break;
                    }
                    text.push(nc);
                }
                if closed && chars.peek() == Some(&'(') {
                    chars.next(); // consume (
                    for nc in chars.by_ref() {
                        if nc == ')' {
                            break;
                        }
                    }
                }
                out.push_str(&text);
            }
            // **, *, _, ` → drop (we're not preserving emphasis here).
            '*' | '_' | '`' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Split into "sentences" by terminal punctuation, keeping the
/// punctuation attached. Falls back to a single chunk if no terminators
/// are found.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？') {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            current.clear();
        }
    }
    let tail = current.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// Animation-flavoured paragraph splitter. Walks the markdown body, takes
/// every non-empty blank-line-separated block, then merges any block
/// shorter than [`PARAGRAPH_MERGE_THRESHOLD`] into the preceding paragraph
/// so the visualised timeline never has a gap.
///
/// Returns the paragraphs as plain (markdown-stripped) text, in body
/// order. If the body has no blank-line separators, the whole body is
/// returned as a single paragraph.
pub fn split_paragraphs(body_md: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for block in body_md.split("\n\n") {
        let plain = strip_markdown(block);
        if plain.is_empty() {
            continue;
        }
        let chars = plain.chars().count();
        if chars < PARAGRAPH_MERGE_THRESHOLD {
            // Merge into the preceding paragraph so the timeline stays
            // continuous. If we're the very first block, keep it on its
            // own — it's the chapter's opening line and the reader still
            // needs *some* visible scene for it.
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(&plain);
                continue;
            }
        }
        out.push(plain);
    }
    out
}

/// Map a character position to a millisecond offset assuming a uniform
/// narration rate over `duration_ms`. `u128` intermediates dodge the
/// overflow that would bite a long book at narration-budget runtime.
pub fn ratio_to_ms(chars_pos: usize, total_chars: usize, duration_ms: u64) -> u64 {
    if total_chars == 0 {
        return 0;
    }
    ((chars_pos as u128 * duration_ms as u128) / total_chars as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_markdown_drops_emphasis_and_links() {
        let s = strip_markdown("# Hello\n\nThis is **bold** and [a link](https://x.com).");
        assert!(!s.contains("**"));
        assert!(!s.contains("https://"));
        assert!(s.contains("Hello"));
        assert!(s.contains("a link"));
    }

    #[test]
    fn strip_markdown_drops_fenced_code() {
        let s = strip_markdown("Before.\n\n```\nlet x = 1;\n```\n\nAfter.");
        assert!(s.contains("Before"));
        assert!(s.contains("After"));
        assert!(!s.contains("let x"));
    }

    #[test]
    fn split_sentences_handles_latin_and_cjk_terminators() {
        let s = split_sentences("Hello world. This is a test! And one more? 你好。");
        assert_eq!(s.len(), 4);
        assert!(s[0].ends_with('.'));
        assert!(s[3].ends_with('。'));
    }

    #[test]
    fn split_sentences_falls_back_to_single_chunk() {
        let s = split_sentences("no terminator here");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn split_paragraphs_merges_short_block_into_predecessor() {
        // Long paragraph (>= 80 chars) followed by a short attribution
        // line — animation needs them on the same scene so the timeline
        // doesn't blink.
        let body = format!(
            "{}\n\n— Author",
            "First long paragraph that comfortably exceeds the threshold for being kept on its own."
        );
        let p = split_paragraphs(&body);
        assert_eq!(p.len(), 1, "short tail should merge into the predecessor");
        assert!(p[0].contains("Author"));
    }

    #[test]
    fn split_paragraphs_keeps_lone_short_first_block() {
        // No predecessor to merge into — keep the short opener as its
        // own paragraph; without it the timeline would start dark.
        let p = split_paragraphs("Short opener.");
        assert_eq!(p, vec!["Short opener.".to_string()]);
    }

    #[test]
    fn split_paragraphs_keeps_two_long_blocks_separate() {
        let a = "A".repeat(120);
        let b = "B".repeat(120);
        let p = split_paragraphs(&format!("{a}\n\n{b}"));
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn ratio_to_ms_distributes_evenly() {
        assert_eq!(ratio_to_ms(0, 100, 10_000), 0);
        assert_eq!(ratio_to_ms(50, 100, 10_000), 5_000);
        assert_eq!(ratio_to_ms(100, 100, 10_000), 10_000);
    }

    #[test]
    fn ratio_to_ms_handles_zero_total() {
        assert_eq!(ratio_to_ms(5, 0, 10_000), 0);
    }
}
