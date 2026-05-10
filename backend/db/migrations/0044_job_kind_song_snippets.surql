-- Widen the `job.kind` allowlist so `JobKind::SongSnippets` clears
-- the SCHEMAFULL ASSERT (last widened in 0032 for animate +
-- animate_chapter). Pure schema rewrite — every prior value still
-- validates.

DEFINE FIELD OVERWRITE kind ON job TYPE string
    ASSERT $value IN [
        "outline",
        "chapters",
        "tts",
        "tts_chapter",
        "post_process",
        "cover",
        "gc",
        "translate",
        "publish_youtube",
        "chapter_paragraphs",
        "animate",
        "animate_chapter",
        "song_snippets"
    ];
