-- Phase 6: cover art.
--
-- Adds a `cover_path` column to audiobook so the API can serve a generated
-- cover image. Path is relative to `Config.storage_path` (typically
-- `<audiobook_id>/cover.png`). Nullable: no cover until the user requests
-- one.

DEFINE FIELD IF NOT EXISTS cover_path ON audiobook TYPE option<string>;
