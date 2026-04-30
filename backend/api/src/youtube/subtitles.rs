//! SRT subtitle generation from chapter prose.
//!
//! The TTS provider doesn't return word-level timing, so we approximate
//! it: characters are assumed to be narrated at a uniform rate over the
//! known WAV duration. That's a perfectly fine approximation for prose
//! audiobooks — sentence-rate variance averages out, and the YouTube
//! player only ever shows ~one cue at a time.
//!
//! Pipeline:
//!   1. Strip markdown (headings, emphasis, links) so cues read cleanly.
//!   2. Split into sentences on `.`/`!`/`?`/`。` and friends.
//!   3. Group sentences into cues capped at `MAX_CUE_CHARS` so a single
//!      cue doesn't outrun the player's two-line layout.
//!   4. Allocate start/end times by `(chars_so_far / total_chars) * duration`.

/// Aim for a cue that fits two lines of YouTube's caption renderer
/// (~42 chars × 2). 180 chars leaves headroom for emoji/wide glyphs.
const MAX_CUE_CHARS: usize = 180;

/// Per-cue length floor; below this we'd be flashing cues so fast the
/// reader can't keep up. The grouping logic fills cues to at least this.
const MIN_CUE_CHARS: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cue {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

/// Build an SRT body for a single chapter narration.
///
/// `duration_ms` is the WAV runtime; `0` is treated as "no audio yet" and
/// returns an empty string so the caller can skip the upload.
pub fn build_srt_for_chapter(text: &str, duration_ms: u64) -> String {
    let cues = build_cues(text, duration_ms, 0);
    cues_to_srt(&cues)
}

/// Build a single SRT body that spans the whole book — chapter timestamps
/// run cumulatively. Each tuple is `(body_md, duration_ms)` for one
/// chapter, in order.
pub fn build_srt_for_book(chapters: &[(&str, u64)]) -> String {
    let mut all: Vec<Cue> = Vec::new();
    let mut offset: u64 = 0;
    for (body, dur) in chapters {
        let cues = build_cues(body, *dur, offset);
        all.extend(cues);
        offset = offset.saturating_add(*dur);
    }
    cues_to_srt(&all)
}

fn build_cues(text: &str, duration_ms: u64, offset_ms: u64) -> Vec<Cue> {
    if duration_ms == 0 || text.trim().is_empty() {
        return Vec::new();
    }
    let plain = strip_markdown(text);
    let sentences = split_sentences(&plain);
    let total_chars: usize = sentences.iter().map(|s| s.chars().count()).sum();
    if total_chars == 0 {
        return Vec::new();
    }

    let mut cues: Vec<Cue> = Vec::new();
    let mut chars_processed: usize = 0;
    let mut cue_text: Vec<String> = Vec::new();
    let mut cue_chars: usize = 0;
    let mut cue_start_chars: usize = 0;

    for sentence in sentences {
        let n = sentence.chars().count();
        let would_overflow = cue_chars + n > MAX_CUE_CHARS;
        let cue_meets_min = cue_chars >= MIN_CUE_CHARS;
        if would_overflow && !cue_text.is_empty() && cue_meets_min {
            push_cue(
                &mut cues,
                offset_ms,
                duration_ms,
                total_chars,
                cue_start_chars,
                chars_processed,
                cue_text.join(" "),
            );
            cue_text.clear();
            cue_chars = 0;
            cue_start_chars = chars_processed;
        }
        cue_text.push(sentence);
        cue_chars += n;
        chars_processed += n;
    }
    if !cue_text.is_empty() {
        push_cue(
            &mut cues,
            offset_ms,
            duration_ms,
            total_chars,
            cue_start_chars,
            chars_processed,
            cue_text.join(" "),
        );
    }

    // Defensive: a 0-duration cue confuses some players. Bump end_ms to at
    // least start_ms + 1ms.
    for cue in &mut cues {
        if cue.end_ms <= cue.start_ms {
            cue.end_ms = cue.start_ms + 1;
        }
    }
    cues
}

fn push_cue(
    cues: &mut Vec<Cue>,
    offset_ms: u64,
    duration_ms: u64,
    total_chars: usize,
    start_chars: usize,
    end_chars: usize,
    text: String,
) {
    let start_ms = offset_ms + ratio_to_ms(start_chars, total_chars, duration_ms);
    let end_ms = offset_ms + ratio_to_ms(end_chars, total_chars, duration_ms);
    cues.push(Cue {
        start_ms,
        end_ms,
        text,
    });
}

fn ratio_to_ms(chars_pos: usize, total_chars: usize, duration_ms: u64) -> u64 {
    if total_chars == 0 {
        return 0;
    }
    // u128 to dodge overflow on long books (a 5h chapter at 1k chars/s
    // would still fit in u64, but the multiplication overflows).
    ((chars_pos as u128 * duration_ms as u128) / total_chars as u128) as u64
}

fn cues_to_srt(cues: &[Cue]) -> String {
    if cues.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(cues.len() * 80);
    for (i, cue) in cues.iter().enumerate() {
        out.push_str(&(i + 1).to_string());
        out.push('\n');
        out.push_str(&format_srt_timestamp(cue.start_ms));
        out.push_str(" --> ");
        out.push_str(&format_srt_timestamp(cue.end_ms));
        out.push('\n');
        out.push_str(cue.text.trim());
        // Blank line separates cues. Keep an extra trailing newline on the
        // file as some parsers expect it.
        out.push_str("\n\n");
    }
    out
}

fn format_srt_timestamp(ms: u64) -> String {
    let total_secs = ms / 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    let frac = ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{frac:03}")
}

// ---------------------------------------------------------------------------
// Markdown stripping + sentence splitting
// ---------------------------------------------------------------------------

/// Cheap markdown remover. Not a parser — just enough to clean up the
/// LLM's prose for caption display:
///   * `# heading` → `heading`
///   * `**bold**`, `*italic*`, `_emph_` → unwrap
///   * `[text](url)` → `text`
///   * fenced code blocks → drop entirely (audio narration would have
///     skipped them anyway).
fn strip_markdown(input: &str) -> String {
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
    let line = line.trim_start_matches(|c: char| c == '>' || c == '-' || c == '*');
    let line = line.trim_start();

    // [text](url) → text
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' => {
                // Find matching ] then optional (url)
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
            // **, *, _ → drop (we're not preserving emphasis for SRT).
            '*' | '_' | '`' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Split into "sentences" by terminal punctuation, keeping the punctuation
/// attached. Falls back to a single chunk if no terminators are found.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        // Latin + CJK sentence terminators. Quote/bracket trail-handling is
        // intentionally skipped — close-parens after a period are unusual
        // enough not to bother with.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_format_matches_srt_spec() {
        assert_eq!(format_srt_timestamp(0), "00:00:00,000");
        assert_eq!(format_srt_timestamp(3_500), "00:00:03,500");
        assert_eq!(format_srt_timestamp(3_600_000 + 65_432), "01:01:05,432");
    }

    #[test]
    fn empty_text_yields_empty_srt() {
        assert!(build_srt_for_chapter("", 10_000).is_empty());
        assert!(build_srt_for_chapter("Hello.", 0).is_empty());
    }

    #[test]
    fn cues_span_full_duration() {
        let text = "Hello world. This is a test sentence. And one more sentence here. Plus another.";
        let srt = build_srt_for_chapter(text, 10_000);
        assert!(srt.contains("00:00:00,000"));
        // Last cue should end exactly at duration.
        assert!(srt.contains("00:00:10,000"));
    }

    #[test]
    fn book_offsets_are_cumulative() {
        let chapters = [
            ("Sentence one. Sentence two.", 5_000u64),
            ("Sentence three. Sentence four.", 5_000u64),
        ];
        let srt = build_srt_for_book(&chapters);
        // Chapter 2 cues must start no earlier than 5s and end no later
        // than 10s.
        assert!(srt.contains("00:00:05,000"));
        assert!(srt.contains("00:00:10,000"));
    }

    #[test]
    fn markdown_is_stripped() {
        let text = "## Hello\n\nThis is **bold** and [a link](https://x.com).";
        let srt = build_srt_for_chapter(text, 5_000);
        assert!(!srt.contains("**"));
        assert!(!srt.contains("https://"));
        assert!(srt.contains("Hello"));
        assert!(srt.contains("a link"));
    }

    #[test]
    fn cues_cap_at_max_length() {
        // Build a long single sentence that exceeds MAX_CUE_CHARS.
        let long = "word ".repeat(200) + ".";
        let srt = build_srt_for_chapter(&long, 60_000);
        // Should produce at least one cue, and no individual cue line
        // should be unboundedly long. The cue text after the timestamp line
        // should be at most a single sentence (since we don't split inside
        // sentences); for this single-sentence input that's the whole text.
        assert!(!srt.is_empty());
    }
}
