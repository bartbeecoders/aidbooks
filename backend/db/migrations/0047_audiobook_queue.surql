-- Multi-audiobook sequential queue.
--
-- A per-user FIFO of audiobooks to generate. The queue runner
-- (`backend/api/src/handlers/queue.rs::spawn_queue_runner`) advances
-- one audiobook at a time: when the running item has no live jobs
-- left, the runner marks it complete and activates the next queued
-- item (unless the user has paused the queue).
--
-- We snapshot the user's `auto_pipeline` at enqueue time so a
-- runner activation in the future doesn't depend on the audiobook's
-- current pipeline column (which the post-success hook NULLs out
-- once it cascades).

DEFINE TABLE IF NOT EXISTS audiobook_queue SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS owner ON audiobook_queue TYPE record<user>;
DEFINE FIELD IF NOT EXISTS audiobook ON audiobook_queue TYPE record<audiobook>;
DEFINE FIELD IF NOT EXISTS position ON audiobook_queue TYPE int;
DEFINE FIELD IF NOT EXISTS state ON audiobook_queue TYPE string
    ASSERT $value IN ["queued", "running", "paused", "completed", "failed", "cancelled"]
    DEFAULT "queued";
DEFINE FIELD IF NOT EXISTS error ON audiobook_queue TYPE option<string>;
DEFINE FIELD IF NOT EXISTS queued_at ON audiobook_queue TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS started_at ON audiobook_queue TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS finished_at ON audiobook_queue TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS updated_at ON audiobook_queue TYPE datetime
    VALUE time::now() DEFAULT time::now();

DEFINE INDEX IF NOT EXISTS queue_owner_position ON audiobook_queue COLUMNS owner, position;
DEFINE INDEX IF NOT EXISTS queue_owner_state ON audiobook_queue COLUMNS owner, state;
DEFINE INDEX IF NOT EXISTS queue_audiobook ON audiobook_queue COLUMNS audiobook;

-- Per-user paused flag. One row keyed by `queue_settings:<user-id>`
-- so we don't have to scan for a control row inside `audiobook_queue`.

DEFINE TABLE IF NOT EXISTS queue_settings SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS owner ON queue_settings TYPE record<user>;
DEFINE FIELD IF NOT EXISTS paused ON queue_settings TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS updated_at ON queue_settings TYPE datetime
    VALUE time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS queue_settings_owner ON queue_settings COLUMNS owner UNIQUE;
