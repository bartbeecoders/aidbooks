-- Widen the prompt_template.role allowlist so the new
-- `outline_songbook` seed template clears the SCHEMAFULL ASSERT in
-- 0003 (last widened in 0038 for voice_extract). Pure schema rewrite —
-- every prior value still validates.
--
-- Split out from 0041 because that one already shipped on dev DBs
-- before the role widening was needed; landing it as 0042 lets the
-- migration runner pick it up on next start without re-applying
-- already-applied work.

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
        "voice_extract",
        "outline_songbook"
    ];
