-- Phase 6: per-language jobs.
--
-- TTS parent + tts_chapter children carry the target language so a single
-- audiobook can have multiple parallel narration jobs (one per language).

DEFINE FIELD IF NOT EXISTS language ON job TYPE option<string>;
