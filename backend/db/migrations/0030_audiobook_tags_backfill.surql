-- Backfill `audiobook.tags` for rows that pre-date migration 0028.
--
-- 0028 defined `tags` as `array<string> DEFAULT []`, but SurrealDB's
-- `DEFAULT` only applies on new INSERTs — existing rows kept `tags =
-- NONE`. Any subsequent UPDATE on those rows then tripped on schemafull
-- validation:
--
--   "Found NONE for field `tags`, but expected a array<string>"
--
-- We deliberately filter on `tags = NONE` so this is a no-op once the
-- backfill has landed (and idempotent across re-runs).

UPDATE audiobook SET tags = [] WHERE tags = NONE;
