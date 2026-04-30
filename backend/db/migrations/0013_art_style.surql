-- Per-audiobook artwork style preference.
--
-- Steers both the cover and per-chapter art generators. Free-text so admins
-- (or future API consumers) can introduce new styles without a migration;
-- the frontend offers a curated dropdown.

DEFINE FIELD IF NOT EXISTS art_style ON audiobook TYPE option<string>
    ASSERT $value = NONE OR string::len($value) <= 60;
