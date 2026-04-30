-- Topic templates: admin-curated starting points for new audiobooks.
--
-- Each template carries a recognisable `title` (shown in the New Audiobook
-- dropdown), a default `topic` body that pre-fills the form, plus optional
-- defaults for genre / length / language. `sort_order` drives the dropdown
-- ordering — lower comes first; `enabled` lets an admin hide a template
-- without deleting it.

DEFINE TABLE IF NOT EXISTS topic_template SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS title ON topic_template TYPE string
    ASSERT string::len($value) > 0 AND string::len($value) <= 120;
DEFINE FIELD IF NOT EXISTS topic ON topic_template TYPE string
    ASSERT string::len($value) > 0 AND string::len($value) <= 1000;
DEFINE FIELD IF NOT EXISTS genre ON topic_template TYPE option<string>;
DEFINE FIELD IF NOT EXISTS length ON topic_template TYPE option<string>
    ASSERT $value = NONE OR $value IN ["short", "medium", "long"];
DEFINE FIELD IF NOT EXISTS language ON topic_template TYPE option<string>;
DEFINE FIELD IF NOT EXISTS sort_order ON topic_template TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS enabled ON topic_template TYPE bool DEFAULT true;
DEFINE FIELD IF NOT EXISTS created_at ON topic_template TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at ON topic_template TYPE datetime
    VALUE time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS topic_template_sort ON topic_template FIELDS sort_order, title;
