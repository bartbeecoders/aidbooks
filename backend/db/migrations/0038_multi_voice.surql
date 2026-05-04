-- Multi-voice narration.
--
-- Optional audiobook setting where dialogue is voiced by per-role
-- voices instead of one narrator. The LLM extract pass tags each
-- chunk of prose with a role (`narrator`, `dialogue_male`,
-- `dialogue_female`); the audio job synthesises each chunk with the
-- role's mapped voice and concatenates the WAVs.
--
-- The `voice_roles` map stays open-ended (FLEXIBLE) so we can grow
-- the role set later (e.g. per-character voices) without another
-- schema migration. Same with `voice_segments` on the chapter row —
-- the cached extraction is opaque to SurrealDB; the audio pipeline
-- is the only consumer.

DEFINE FIELD IF NOT EXISTS multi_voice_enabled ON audiobook
    TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS voice_roles ON audiobook
    FLEXIBLE TYPE option<object>;

-- Cached output of the voice-extract LLM pass — `[{role, text}, ...]`.
-- `NONE` until the first narration with multi-voice enabled. Cleared
-- on translate (translated chapters get re-extracted automatically).
DEFINE FIELD IF NOT EXISTS voice_segments ON chapter
    FLEXIBLE TYPE option<array<object>>;

-- Widen the prompt_template.role allowlist so the new
-- `voice_extract` template clears the SCHEMAFULL ASSERT in 0003 (last
-- widened in 0035 for manim_code). Same shape as the prior widenings:
-- pure schema rewrite, every prior value still validates.
DEFINE FIELD OVERWRITE role ON prompt_template TYPE string
    ASSERT $value IN [
        "outline",
        "chapter",
        "random_topic",
        "moderation",
        "title",
        "cover",
        "paragraph_image",
        "translate",
        "scene_extract",
        "paragraph_visual",
        "manim_code",
        "voice_extract"
    ];
