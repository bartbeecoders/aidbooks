-- Phase 6: per-audiobook language.
--
-- Drives both LLM content generation (outline + chapter prose) and TTS
-- narration. BCP-47 codes ("en", "nl", "de", ...). Existing rows fall back
-- to "en" via the DEFAULT clause; new rows use whatever the API was given.

DEFINE FIELD IF NOT EXISTS language ON audiobook TYPE string DEFAULT "en";

-- Backfill any rows created before this migration so the deserialiser
-- doesn't have to handle a NULL.
UPDATE audiobook SET language = "en" WHERE language = NONE;
