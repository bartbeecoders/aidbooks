-- Phase 3: content generation.
--
-- Extends the audiobook/chapter lifecycle with in-flight statuses, and adds
-- two new tables:
--   * prompt_template — versioned, admin-editable prompt body per role
--   * generation_event — append-only log of every LLM call for cost + audit

-- Expand audiobook status states to cover the in-flight lifecycle.
DEFINE FIELD OVERWRITE status ON audiobook TYPE string
    ASSERT $value IN [
        "draft",
        "outline_pending",
        "outline_ready",
        "chapters_running",
        "text_ready",
        "audio_ready",
        "failed"
    ]
    DEFAULT "draft";

-- Chapters get a "running" state for in-flight LLM calls.
DEFINE FIELD OVERWRITE status ON chapter TYPE string
    ASSERT $value IN [
        "pending",
        "running",
        "text_ready",
        "audio_ready",
        "failed"
    ]
    DEFAULT "pending";

-- Prompt templates --------------------------------------------------------
DEFINE TABLE IF NOT EXISTS prompt_template SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS role ON prompt_template TYPE string
    ASSERT $value IN ["outline", "chapter", "random_topic", "moderation", "title"];
DEFINE FIELD IF NOT EXISTS body ON prompt_template TYPE string;
DEFINE FIELD IF NOT EXISTS version ON prompt_template TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS active ON prompt_template TYPE bool DEFAULT true;
DEFINE FIELD IF NOT EXISTS variables ON prompt_template TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS created_at ON prompt_template TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS prompt_active_role ON prompt_template FIELDS role, active;

-- Per-LLM-call cost + audit log ------------------------------------------
DEFINE TABLE IF NOT EXISTS generation_event SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS user ON generation_event TYPE record<user>;
DEFINE FIELD IF NOT EXISTS audiobook ON generation_event TYPE option<record<audiobook>>;
DEFINE FIELD IF NOT EXISTS llm ON generation_event TYPE record<llm>;
DEFINE FIELD IF NOT EXISTS role ON generation_event TYPE string;
DEFINE FIELD IF NOT EXISTS prompt_tokens ON generation_event TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS completion_tokens ON generation_event TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS cost_usd ON generation_event TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS success ON generation_event TYPE bool DEFAULT true;
DEFINE FIELD IF NOT EXISTS error ON generation_event TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON generation_event TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS gen_event_by_user ON generation_event FIELDS user;
DEFINE INDEX IF NOT EXISTS gen_event_by_audiobook ON generation_event FIELDS audiobook;
