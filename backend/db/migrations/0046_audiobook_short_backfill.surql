-- Backfill nullable-when-old audiobook + topic_template fields that
-- pre-date their DEFAULT-bearing field definitions.
--
-- SurrealDB `DEFAULT` only fires on INSERT, so rows created before a
-- given migration ran kept the field as `NONE`. Any subsequent UPDATE
-- on those rows then trips the schemafull check on *every* non-option
-- field that's still NONE, e.g.:
--
--   "Found NONE for field `is_songbook`, with record `audiobook:…`,
--    but expected a bool"
--
-- The previous narrowly-scoped variant of this migration only set
-- `is_short` and `narration_intensity`, but the UPDATE still validated
-- the whole row and tripped on `is_songbook`. We now coalesce every
-- such field at once (`?? <default>`) inside one UPDATE so the
-- post-UPDATE validation finds a fully-typed row regardless of which
-- fields were NONE on the way in.
--
-- WHERE filters keep the migration a no-op on already-clean rows so
-- `updated_at` isn't bumped on rows that don't need touching, and
-- re-runs are idempotent.

UPDATE audiobook SET
    is_short            = is_short            ?? false,
    is_songbook         = is_songbook         ?? false,
    snippet_count       = snippet_count       ?? 0,
    narration_intensity = narration_intensity ?? [],
    multi_voice_enabled = multi_voice_enabled ?? false,
    language            = language            ?? "en"
WHERE is_short            = NONE
   OR is_songbook         = NONE
   OR snippet_count       = NONE
   OR narration_intensity = NONE
   OR multi_voice_enabled = NONE
   OR language            = NONE;

UPDATE topic_template SET
    is_short = is_short ?? false
WHERE is_short = NONE;
