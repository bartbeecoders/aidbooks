-- Audiobook ideas — owner-scoped backlog of topics the user wants to
-- turn into audiobooks later.
--
-- Each row is a freeform `title` plus an `audiobook_prompt` (the text
-- handed to the create flow when the user clicks "Generate"). The
-- `source` column tracks where the idea came from so the UI can mark
-- LLM-suggested trends visually distinct from manual entries; it
-- isn't enforced by an ASSERT so we can add new sources without a
-- schema bump.

DEFINE TABLE IF NOT EXISTS idea SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS owner ON idea TYPE record<user>;
DEFINE FIELD IF NOT EXISTS title ON idea TYPE string
    ASSERT string::len($value) >= 1 AND string::len($value) <= 300;
DEFINE FIELD IF NOT EXISTS audiobook_prompt ON idea TYPE string
    ASSERT string::len($value) <= 4000 DEFAULT "";
DEFINE FIELD IF NOT EXISTS status ON idea TYPE string
    ASSERT $value IN ["pending", "in_progress", "completed"]
    DEFAULT "pending";
-- Free-form provenance label. `manual` for user-typed entries,
-- `trend` for LLM-suggested trends. Left as a plain string so future
-- sources slot in without a schema migration.
DEFINE FIELD IF NOT EXISTS source ON idea TYPE string DEFAULT "manual";
DEFINE FIELD IF NOT EXISTS created_at ON idea TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS completed_at ON idea TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS updated_at ON idea TYPE datetime
    VALUE time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS idea_by_owner ON idea FIELDS owner;
