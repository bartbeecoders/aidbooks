-- Per-chapter artwork path.
--
-- The `chapter_art_path` column was previously declared in a stray
-- `0005_chapter_art.surql` file that never made it into the migration
-- runner, so SCHEMAFULL writes to it failed at runtime. Re-declared here
-- with `IF NOT EXISTS` so re-running on a partially-migrated DB is safe.

DEFINE FIELD IF NOT EXISTS chapter_art_path ON chapter TYPE option<string>;
