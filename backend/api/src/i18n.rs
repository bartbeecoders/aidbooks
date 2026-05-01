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

/// Section + footer copy used to assemble YouTube video descriptions in
/// the language being published. Falls back to English for unknown
/// codes — that's the worst case where a viewer sees one English label
/// surrounded by translated prose, which is still better than emitting
/// only English copy on a translated upload.
pub struct DescriptionLabels {
    pub chapters_heading: &'static str,
    pub genre_label: &'static str,
    pub generated_with: &'static str,
    /// Format string with two `{}` placeholders: chapter number, then
    /// total chapter count (e.g. `"Chapter {} of {}"` → `"Chapter 3 of 6"`).
    pub chapter_of: fn(u32, u32) -> String,
    /// Format string with one `{}` placeholder for the book title
    /// (e.g. `"From {}."` → `"From Quiet Mornings."`).
    pub from_book: fn(&str) -> String,
}

pub fn description_labels(code: &str) -> DescriptionLabels {
    match code {
        "nl" => DescriptionLabels {
            chapters_heading: "Hoofdstukken:",
            genre_label: "Genre:",
            generated_with: "Gegenereerd met AidBooks.",
            chapter_of: |n, total| format!("Hoofdstuk {n} van {total}"),
            from_book: |title| format!("Uit {title}."),
        },
        "fr" => DescriptionLabels {
            chapters_heading: "Chapitres :",
            genre_label: "Genre :",
            generated_with: "Généré avec AidBooks.",
            chapter_of: |n, total| format!("Chapitre {n} sur {total}"),
            from_book: |title| format!("Extrait de {title}."),
        },
        "de" => DescriptionLabels {
            chapters_heading: "Kapitel:",
            genre_label: "Genre:",
            generated_with: "Erstellt mit AidBooks.",
            chapter_of: |n, total| format!("Kapitel {n} von {total}"),
            from_book: |title| format!("Aus {title}."),
        },
        "es" => DescriptionLabels {
            chapters_heading: "Capítulos:",
            genre_label: "Género:",
            generated_with: "Generado con AidBooks.",
            chapter_of: |n, total| format!("Capítulo {n} de {total}"),
            from_book: |title| format!("De {title}."),
        },
        "it" => DescriptionLabels {
            chapters_heading: "Capitoli:",
            genre_label: "Genere:",
            generated_with: "Generato con AidBooks.",
            chapter_of: |n, total| format!("Capitolo {n} di {total}"),
            from_book: |title| format!("Da {title}."),
        },
        "pt" => DescriptionLabels {
            chapters_heading: "Capítulos:",
            genre_label: "Gênero:",
            generated_with: "Gerado com AidBooks.",
            chapter_of: |n, total| format!("Capítulo {n} de {total}"),
            from_book: |title| format!("De {title}."),
        },
        "ru" => DescriptionLabels {
            chapters_heading: "Главы:",
            genre_label: "Жанр:",
            generated_with: "Сгенерировано в AidBooks.",
            chapter_of: |n, total| format!("Глава {n} из {total}"),
            from_book: |title| format!("Из «{title}»."),
        },
        "zh" => DescriptionLabels {
            chapters_heading: "章节：",
            genre_label: "类型：",
            generated_with: "由 AidBooks 生成。",
            chapter_of: |n, total| format!("第 {n} 章 / 共 {total} 章"),
            from_book: |title| format!("出自《{title}》。"),
        },
        "ja" => DescriptionLabels {
            chapters_heading: "チャプター:",
            genre_label: "ジャンル:",
            generated_with: "AidBooks で生成。",
            chapter_of: |n, total| format!("第{n}章 / 全{total}章"),
            from_book: |title| format!("『{title}』より。"),
        },
        "ko" => DescriptionLabels {
            chapters_heading: "챕터:",
            genre_label: "장르:",
            generated_with: "AidBooks로 생성됨.",
            chapter_of: |n, total| format!("{total}장 중 {n}장"),
            from_book: |title| format!("《{title}》에서."),
        },
        // English + unknown.
        _ => DescriptionLabels {
            chapters_heading: "Chapters:",
            genre_label: "Genre:",
            generated_with: "Generated with AidBooks.",
            chapter_of: |n, total| format!("Chapter {n} of {total}"),
            from_book: |title| format!("From {title}."),
        },
    }
}
