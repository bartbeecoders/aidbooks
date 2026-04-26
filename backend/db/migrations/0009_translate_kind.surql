-- Phase 6: translate jobs.
--
-- Widen the `job.kind` enum so the runtime can persist
-- `JobKind::Translate` rows. The new kind runs an LLM-based chapter
-- translation in the background; sync was hitting the request-level timeout
-- on books with more than a few chapters.

DEFINE FIELD OVERWRITE kind ON job TYPE string
    ASSERT $value IN [
        "outline",
        "chapters",
        "tts",
        "tts_chapter",
        "post_process",
        "cover",
        "gc",
        "translate"
    ];
