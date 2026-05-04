-- Phase H — bespoke Manim code generator role.
--
-- Adds `manim_code` to the prompt_template.role allowlist so the new
-- code-gen prompt clears the SCHEMAFULL ASSERT defined in 0003 (and
-- last widened in 0034 for paragraph_visual). Same shape as 0034:
-- pure schema rewrite, every prior value still validates.
--
-- The `manim_code` LlmRole and the `manim_code` literal that lands in
-- `llm.default_for` need no migration: 0001 declares
-- `default_for ON llm TYPE array<string> DEFAULT []` with no element
-- ASSERT, so any string is already accepted there.
--
-- Paragraphs gain an optional `manim_code` field. The chapter row
-- already stores paragraphs as `FLEXIBLE TYPE option<array<object>>`
-- (0023), so the new field needs no column add — it just slots into
-- the flexible object alongside `visual_kind` and `visual_params`.
-- This file documents the intent; the runtime serializer in
-- `handlers/audiobook.rs` is what actually writes the field.

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
        "manim_code"
    ];
