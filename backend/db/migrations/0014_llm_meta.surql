-- LLM metadata: function, language coverage, picker priority.
--
-- * `function` — what kind of model this is (`text`, `image`, …). Drives UI
--   grouping + picker sanity checks. Free-text so future categories
--   (audio, embedding, …) land without a migration.
-- * `languages` — BCP-47 codes the model handles well. Empty array means
--   "any language"; otherwise the picker prefers a model whose list
--   includes the target language.
-- * `priority` — lower wins. Used to deterministically pick between two
--   otherwise-equivalent models (same function, both default-for the role,
--   both language-compatible). New rows default to 100 so admins can
--   promote/demote around them.
--
-- Existing rows aren't backfilled here — `function` is a reserved
-- namespace prefix in SurrealDB's expression parser (`function::name(…)`),
-- so `WHERE function = NONE` won't parse. We don't need the backfill
-- anyway: the Rust DTO uses `#[serde(default)]` for `function`/`languages`
-- and `default_priority` (= 100) for `priority`, so old rows present as
-- `function: None`, `languages: []`, `priority: 100` to the picker.

DEFINE FIELD IF NOT EXISTS function ON llm TYPE option<string>
    ASSERT $value = NONE OR string::len($value) <= 40;
DEFINE FIELD IF NOT EXISTS languages ON llm TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS priority ON llm TYPE int DEFAULT 100;
