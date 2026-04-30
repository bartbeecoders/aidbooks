-- Phase 8: YouTube publishing.
--
-- Three new tables:
--   * youtube_account     — one OAuth-connected channel per user.
--   * youtube_publication — one row per (audiobook, language) ever published.
--   * oauth_state         — short-TTL anti-CSRF tokens for the consent dance.
--
-- Plus a wider `job.kind` ASSERT so the runtime can persist `publish_youtube`.

-- --- youtube_account -----------------------------------------------------
DEFINE TABLE IF NOT EXISTS youtube_account SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS owner ON youtube_account TYPE record<user>;
DEFINE FIELD IF NOT EXISTS channel_id ON youtube_account TYPE string;
DEFINE FIELD IF NOT EXISTS channel_title ON youtube_account TYPE string;
-- AEAD-encrypted refresh token; never store the plaintext.
DEFINE FIELD IF NOT EXISTS refresh_token_enc ON youtube_account TYPE string;
DEFINE FIELD IF NOT EXISTS scopes ON youtube_account TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS connected_at ON youtube_account TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS yt_account_owner ON youtube_account FIELDS owner UNIQUE;

-- --- youtube_publication -------------------------------------------------
DEFINE TABLE IF NOT EXISTS youtube_publication SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS audiobook ON youtube_publication TYPE record<audiobook>;
DEFINE FIELD IF NOT EXISTS language ON youtube_publication TYPE string;
DEFINE FIELD IF NOT EXISTS video_id ON youtube_publication TYPE option<string>;
DEFINE FIELD IF NOT EXISTS privacy_status ON youtube_publication TYPE string
    ASSERT $value IN ["private", "unlisted", "public"];
DEFINE FIELD IF NOT EXISTS video_url ON youtube_publication TYPE option<string>;
DEFINE FIELD IF NOT EXISTS published_at ON youtube_publication TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS last_error ON youtube_publication TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON youtube_publication TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at ON youtube_publication TYPE datetime
    VALUE time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS yt_publication_unique ON youtube_publication
    FIELDS audiobook, language UNIQUE;
DEFINE INDEX IF NOT EXISTS yt_publication_by_audiobook ON youtube_publication
    FIELDS audiobook;

-- --- oauth_state ---------------------------------------------------------
-- Short-lived (10 min) anti-CSRF + user-binding token for OAuth redirects.
-- The browser drops `Authorization` headers across the Google round-trip, so
-- we identify the returning user by looking up `state`.
DEFINE TABLE IF NOT EXISTS oauth_state SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS user ON oauth_state TYPE record<user>;
DEFINE FIELD IF NOT EXISTS provider ON oauth_state TYPE string;
DEFINE FIELD IF NOT EXISTS state ON oauth_state TYPE string;
DEFINE FIELD IF NOT EXISTS expires_at ON oauth_state TYPE datetime;
DEFINE FIELD IF NOT EXISTS created_at ON oauth_state TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS oauth_state_lookup ON oauth_state FIELDS state UNIQUE;

-- --- widen job.kind ------------------------------------------------------
DEFINE FIELD OVERWRITE kind ON job TYPE string
    ASSERT $value IN [
        "outline",
        "chapters",
        "tts",
        "tts_chapter",
        "post_process",
        "cover",
        "gc",
        "translate",
        "publish_youtube"
    ];
