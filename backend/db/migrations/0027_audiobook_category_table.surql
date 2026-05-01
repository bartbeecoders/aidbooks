-- Curated list of audiobook categories. Admin manages the catalog;
-- users (and the create/edit forms) pick from it. The `audiobook.category`
-- field stays a free-text string for write simplicity, validated against
-- this table at API write time.
--
-- Renames cascade: editing `name` here triggers an UPDATE across
-- audiobook rows whose category matches the old name. Deletes blank the
-- field on matching audiobook rows (move-to-uncategorized).

DEFINE TABLE IF NOT EXISTS audiobook_category SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name ON audiobook_category TYPE string
    ASSERT string::len($value) >= 1 AND string::len($value) <= 60;
DEFINE FIELD IF NOT EXISTS created_at ON audiobook_category TYPE datetime
    DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at ON audiobook_category TYPE datetime
    DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS audiobook_category_name_unique
    ON audiobook_category FIELDS name UNIQUE;
