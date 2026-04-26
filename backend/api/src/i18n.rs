//! BCP-47 → human-readable label.
//!
//! The LLM prompts inject the language name (e.g. "Dutch") rather than the
//! code, since prose-grade models follow natural-language directions much
//! more reliably than they follow ISO codes. Unknown codes pass through
//! unchanged so the model still gets a hint to work with.

pub fn label(code: &str) -> &str {
    match code {
        "en" => "English",
        "nl" => "Dutch",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "zh" => "Chinese",
        "ja" => "Japanese",
        "ko" => "Korean",
        other => other,
    }
}
