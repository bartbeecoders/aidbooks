-- Per-paragraph illustration support.
--
-- Replaces the per-chapter `chapter_image_count` knob added in 0022 with a
-- finer-grained, paragraph-level model:
--
--   audiobook.images_per_paragraph  — how many tiles to generate for each
--                                     visual paragraph (0..=3). 0 = legacy
--                                     "chapter cover only" behaviour.
--   chapter.paragraphs              — array of paragraph objects produced
--                                     by the splitter + extract LLM pass:
--     [
--       {
--         "index": 0,
--         "text":  "<paragraph body, markdown>",
--         "char_count": 312,
--         "scene_description": "<vivid scene>"  // None for non-visual blocks
--         "image_paths": ["<rel path>", ...]
--       },
--       ...
--     ]
--
-- 0022's `chapter_image_count` and `chapter_image_paths` are intentionally
-- left declared (forward-only) — new books just stop using them.
--
-- Widen `job.kind` so the runtime can persist the new
-- `chapter_paragraphs` orchestrator job (extract + fan-out).

DEFINE FIELD IF NOT EXISTS images_per_paragraph ON audiobook TYPE option<int>
    ASSERT $value = NONE OR ($value >= 0 AND $value <= 3);

-- FLEXIBLE so we can grow paragraph object fields without one DEFINE FIELD
-- per inner key. Same pattern we use for `audiobook.auto_pipeline` (0021).
DEFINE FIELD IF NOT EXISTS paragraphs ON chapter FLEXIBLE TYPE option<array<object>>;

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
        "chapter_paragraphs"
    ];
