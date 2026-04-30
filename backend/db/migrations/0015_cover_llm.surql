-- Per-audiobook cover/chapter-art LLM override.
--
-- When set, both the cover regeneration and the per-chapter art handler
-- use this exact LLM instead of running the picker. NONE means "fall back
-- to whichever LLM is marked default_for cover_art".

DEFINE FIELD IF NOT EXISTS cover_llm_id ON audiobook TYPE option<string>
    ASSERT $value = NONE OR string::len($value) <= 64;
