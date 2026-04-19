-- ListenAI schema bootstrap.
-- DEFINE ... IF NOT EXISTS makes this safe to re-run; the _migrations table
-- tracks which files have been executed so we can add forward migrations
-- later without relying on idempotency alone.

-- --- users ---------------------------------------------------------------
DEFINE TABLE IF NOT EXISTS user SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS email ON user TYPE string
    ASSERT string::is::email($value);
DEFINE FIELD IF NOT EXISTS display_name ON user TYPE string;
DEFINE FIELD IF NOT EXISTS role ON user TYPE string
    ASSERT $value IN ["user", "admin"] DEFAULT "user";
DEFINE FIELD IF NOT EXISTS tier ON user TYPE string
    ASSERT $value IN ["free", "pro"] DEFAULT "free";
DEFINE FIELD IF NOT EXISTS password_hash ON user TYPE option<string>;
DEFINE FIELD IF NOT EXISTS email_verified_at ON user TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS created_at ON user TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS user_email_unique ON user FIELDS email UNIQUE;

-- --- LLMs ----------------------------------------------------------------
DEFINE TABLE IF NOT EXISTS llm SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name ON llm TYPE string;
DEFINE FIELD IF NOT EXISTS provider ON llm TYPE string
    ASSERT $value IN ["open_router"];
DEFINE FIELD IF NOT EXISTS model_id ON llm TYPE string;
DEFINE FIELD IF NOT EXISTS context_window ON llm TYPE int DEFAULT 128000;
DEFINE FIELD IF NOT EXISTS cost_prompt_per_1k ON llm TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS cost_completion_per_1k ON llm TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS enabled ON llm TYPE bool DEFAULT true;
DEFINE FIELD IF NOT EXISTS default_for ON llm TYPE array<string> DEFAULT [];

-- --- voices --------------------------------------------------------------
DEFINE TABLE IF NOT EXISTS voice SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name ON voice TYPE string;
DEFINE FIELD IF NOT EXISTS provider ON voice TYPE string
    ASSERT $value IN ["xai"];
DEFINE FIELD IF NOT EXISTS provider_voice_id ON voice TYPE string;
DEFINE FIELD IF NOT EXISTS gender ON voice TYPE string
    ASSERT $value IN ["female", "male", "neutral"];
DEFINE FIELD IF NOT EXISTS accent ON voice TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS language ON voice TYPE string DEFAULT "en";
DEFINE FIELD IF NOT EXISTS sample_url ON voice TYPE option<string>;
DEFINE FIELD IF NOT EXISTS enabled ON voice TYPE bool DEFAULT true;
DEFINE FIELD IF NOT EXISTS premium_only ON voice TYPE bool DEFAULT false;

-- --- audiobooks ----------------------------------------------------------
DEFINE TABLE IF NOT EXISTS audiobook SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS owner ON audiobook TYPE record<user>;
DEFINE FIELD IF NOT EXISTS title ON audiobook TYPE string;
DEFINE FIELD IF NOT EXISTS topic ON audiobook TYPE string;
DEFINE FIELD IF NOT EXISTS genre ON audiobook TYPE option<string>;
DEFINE FIELD IF NOT EXISTS length ON audiobook TYPE string
    ASSERT $value IN ["short", "medium", "long"] DEFAULT "short";
DEFINE FIELD IF NOT EXISTS primary_voice ON audiobook TYPE option<record<voice>>;
DEFINE FIELD IF NOT EXISTS status ON audiobook TYPE string
    ASSERT $value IN ["draft", "outline_ready", "text_ready", "audio_ready", "failed"]
    DEFAULT "draft";
DEFINE FIELD IF NOT EXISTS created_at ON audiobook TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at ON audiobook TYPE datetime
    VALUE time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS audiobook_by_owner ON audiobook FIELDS owner;

-- --- chapters ------------------------------------------------------------
DEFINE TABLE IF NOT EXISTS chapter SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS audiobook ON chapter TYPE record<audiobook>;
DEFINE FIELD IF NOT EXISTS number ON chapter TYPE int;
DEFINE FIELD IF NOT EXISTS title ON chapter TYPE string;
DEFINE FIELD IF NOT EXISTS synopsis ON chapter TYPE option<string>;
DEFINE FIELD IF NOT EXISTS target_words ON chapter TYPE option<int>;
DEFINE FIELD IF NOT EXISTS body_md ON chapter TYPE option<string>;
DEFINE FIELD IF NOT EXISTS audio_path ON chapter TYPE option<string>;
DEFINE FIELD IF NOT EXISTS duration_ms ON chapter TYPE option<int>;
DEFINE FIELD IF NOT EXISTS status ON chapter TYPE string
    ASSERT $value IN ["pending", "text_ready", "audio_ready", "failed"]
    DEFAULT "pending";
DEFINE INDEX IF NOT EXISTS chapter_by_audiobook ON chapter FIELDS audiobook, number UNIQUE;

-- --- jobs ----------------------------------------------------------------
DEFINE TABLE IF NOT EXISTS job SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS kind ON job TYPE string
    ASSERT $value IN ["outline", "chapters", "tts", "post_process", "cover"];
DEFINE FIELD IF NOT EXISTS audiobook ON job TYPE option<record<audiobook>>;
DEFINE FIELD IF NOT EXISTS status ON job TYPE string
    ASSERT $value IN ["queued", "running", "completed", "failed", "throttled", "dead"]
    DEFAULT "queued";
DEFINE FIELD IF NOT EXISTS progress_pct ON job TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS attempts ON job TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS last_error ON job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS queued_at ON job TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS started_at ON job TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS finished_at ON job TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS payload ON job TYPE option<object>;
DEFINE INDEX IF NOT EXISTS job_status_idx ON job FIELDS status;
