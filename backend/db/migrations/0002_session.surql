-- Refresh-token sessions. One row per issued refresh token. Rotation
-- creates a new row and marks the old one revoked; reuse of a revoked
-- token is a signal to nuke all of the user's other sessions too.

DEFINE TABLE IF NOT EXISTS session SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS user               ON session TYPE record<user>;
DEFINE FIELD IF NOT EXISTS refresh_token_hash ON session TYPE string;
DEFINE FIELD IF NOT EXISTS user_agent         ON session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS ip                 ON session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS expires_at         ON session TYPE datetime;
DEFINE FIELD IF NOT EXISTS revoked_at         ON session TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS created_at         ON session TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();

DEFINE INDEX IF NOT EXISTS session_by_user ON session FIELDS user;
DEFINE INDEX IF NOT EXISTS session_refresh_hash_unique
    ON session FIELDS refresh_token_hash UNIQUE;
