-- Phase 6: per-language chapter rows.
--
-- A single audiobook can now be available in multiple languages. Each
-- language version is a parallel set of chapter rows: same `number`,
-- different `language`, with its own translated `title`/`synopsis`/`body_md`
-- and (after narration) its own `audio_path`.
--
-- The previous unique constraint `(audiobook, number)` is replaced with
-- `(audiobook, number, language)` so the new rows can coexist.

DEFINE FIELD IF NOT EXISTS language ON chapter TYPE string DEFAULT "en";

-- Backfill: any existing chapter without a language was created before this
-- migration ran, so it belongs to the audiobook's primary language.
UPDATE chapter SET language = "en" WHERE language = NONE;

-- Swap the unique index. Surreal doesn't have ALTER INDEX, so REMOVE +
-- DEFINE. The old name is dropped explicitly to avoid two unique constraints
-- fighting over the same fields.
REMOVE INDEX IF EXISTS chapter_by_audiobook ON chapter;
DEFINE INDEX IF NOT EXISTS chapter_by_audiobook_lang
    ON chapter FIELDS audiobook, number, language UNIQUE;
