-- Phase 11: podcasts.
--
-- A podcast is an owner-scoped grouping of audiobooks. Each podcast has a
-- title, description, and an AI-generated cover image. Audiobooks reference
-- a podcast by id; deleting a podcast clears the reference (move-to-none).
--
-- Future work: when YouTube is connected, creating a podcast will also
-- create a real YouTube playlist on the user's channel and store its id
-- here so publish jobs can drop new videos into it automatically.

DEFINE TABLE IF NOT EXISTS podcast SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS owner ON podcast TYPE record<user>;
DEFINE FIELD IF NOT EXISTS title ON podcast TYPE string
    ASSERT string::len($value) >= 1 AND string::len($value) <= 200;
DEFINE FIELD IF NOT EXISTS description ON podcast TYPE string
    ASSERT string::len($value) <= 4000 DEFAULT "";
-- Relative path under `Config.storage_path` to the cover image; served via
-- `GET /podcasts/:id/image`. NONE until the user generates one.
DEFINE FIELD IF NOT EXISTS image_path ON podcast TYPE option<string>;
-- Reserved for Phase 2 — the YouTube playlist id we create on the user's
-- channel. Stays NONE until the YouTube wiring lands.
DEFINE FIELD IF NOT EXISTS youtube_playlist_id ON podcast TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON podcast TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at ON podcast TYPE datetime
    VALUE time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS podcast_by_owner ON podcast FIELDS owner;

-- Optional reference from audiobook → podcast. NONE = unassigned.
DEFINE FIELD IF NOT EXISTS podcast ON audiobook TYPE option<record<podcast>>;
