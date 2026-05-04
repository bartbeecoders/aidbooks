-- Animation feature (feature/animation, Phases A–E).
--
-- Two new `JobKind` variants need to clear the `job.kind` ASSERT:
--   * `animate`         — parent fan-out job, one per audiobook + language.
--   * `animate_chapter` — per-chapter render that drives the Node sidecar.
--
-- Pure schema widen — no data migration. Pre-existing rows still validate
-- because we only add to the allowlist, never remove.

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
        "animate_chapter"
    ];
