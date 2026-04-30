-- YouTube playlist publishing.
--
-- Extend `youtube_publication` with a publication mode plus playlist
-- bookkeeping, and add a new `youtube_publication_video` table that holds one
-- row per chapter video when `mode = "playlist"`. The single-video path is
-- unchanged (mode defaults to "single").

-- --- youtube_publication: mode + playlist fields -------------------------
DEFINE FIELD IF NOT EXISTS mode ON youtube_publication TYPE string
    DEFAULT "single"
    ASSERT $value IN ["single", "playlist"];
DEFINE FIELD IF NOT EXISTS playlist_id ON youtube_publication TYPE option<string>;
DEFINE FIELD IF NOT EXISTS playlist_url ON youtube_publication TYPE option<string>;

-- --- youtube_publication_video -------------------------------------------
-- One row per chapter when the parent publication uses playlist mode.
-- `chapter_number` is 1-based and matches `chapter.number`.
DEFINE TABLE IF NOT EXISTS youtube_publication_video SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS publication ON youtube_publication_video
    TYPE record<youtube_publication>;
DEFINE FIELD IF NOT EXISTS chapter_number ON youtube_publication_video TYPE int;
DEFINE FIELD IF NOT EXISTS title ON youtube_publication_video TYPE string;
DEFINE FIELD IF NOT EXISTS video_id ON youtube_publication_video TYPE option<string>;
DEFINE FIELD IF NOT EXISTS video_url ON youtube_publication_video TYPE option<string>;
DEFINE FIELD IF NOT EXISTS published_at ON youtube_publication_video TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS last_error ON youtube_publication_video TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON youtube_publication_video TYPE datetime
    VALUE $before OR time::now() DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at ON youtube_publication_video TYPE datetime
    VALUE time::now() DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS yt_pub_video_unique ON youtube_publication_video
    FIELDS publication, chapter_number UNIQUE;
DEFINE INDEX IF NOT EXISTS yt_pub_video_by_publication ON youtube_publication_video
    FIELDS publication;
