-- Phase 5: durable job orchestration.
--
-- Extends the `job` table introduced in 0001 with the bookkeeping a real
-- worker needs (owner, worker_id, retries, backoff, fan-out parent) and
-- adds a `request_idempotency` table so mutating endpoints can carry
-- `Idempotency-Key` headers without risking double-enqueue on client retry.

-- --- job bookkeeping ------------------------------------------------------
-- Who queued it (for per-user quota + audit); nullable for system-origin
-- jobs like the nightly GC task.
DEFINE FIELD IF NOT EXISTS user ON job TYPE option<record<user>>;

-- Tts fan-out: a "tts" parent spawns one "tts_chapter" child per chapter.
-- Parent tracks aggregate progress; children carry the real work.
DEFINE FIELD IF NOT EXISTS parent ON job TYPE option<record<job>>;
DEFINE FIELD IF NOT EXISTS chapter_number ON job TYPE option<int>;

-- Leased-worker id set atomically at pickup; cleared on terminal state.
DEFINE FIELD IF NOT EXISTS worker_id ON job TYPE option<string>;

-- Retries with exponential backoff. `not_before` is the earliest time
-- this job may be picked up again after a failure.
DEFINE FIELD IF NOT EXISTS max_attempts ON job TYPE int DEFAULT 3;
DEFINE FIELD IF NOT EXISTS not_before ON job TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at ON job TYPE datetime
    VALUE time::now() DEFAULT time::now();

-- Widen kind constraint so fan-out children fit the schema.
DEFINE FIELD OVERWRITE kind ON job TYPE string
    ASSERT $value IN [
        "outline",
        "chapters",
        "tts",
        "tts_chapter",
        "post_process",
        "cover",
        "gc"
    ];

-- Composite index for the pickup query:
--   WHERE status = 'queued' AND kind IN (...) AND not_before <= now
-- ORDER BY queued_at ASC
DEFINE INDEX IF NOT EXISTS job_pickup_idx ON job FIELDS status, kind, not_before;
DEFINE INDEX IF NOT EXISTS job_by_parent_idx ON job FIELDS parent;
DEFINE INDEX IF NOT EXISTS job_by_audiobook_idx ON job FIELDS audiobook;

-- --- idempotency keys ----------------------------------------------------
-- Stores the first successful response for 24h so retries from the client
-- don't double-enqueue. Scoped per (user, key) — different users can't
-- collide, and an unauthenticated key is never stored.
DEFINE TABLE IF NOT EXISTS request_idempotency SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS user ON request_idempotency TYPE record<user>;
DEFINE FIELD IF NOT EXISTS key ON request_idempotency TYPE string;
DEFINE FIELD IF NOT EXISTS method ON request_idempotency TYPE string;
DEFINE FIELD IF NOT EXISTS path ON request_idempotency TYPE string;
DEFINE FIELD IF NOT EXISTS status_code ON request_idempotency TYPE int;
DEFINE FIELD IF NOT EXISTS response_body ON request_idempotency TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON request_idempotency TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS expires_at ON request_idempotency TYPE datetime;
DEFINE INDEX IF NOT EXISTS idempotency_user_key ON request_idempotency
    FIELDS user, key UNIQUE;
