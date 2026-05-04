-- Singleton settings for YouTube publishing.
--
-- Currently holds one toggle: `include_credits`. When true, the publisher
-- appends a compact "Models used" block to every YouTube description it
-- generates (built from the audiobook's `generation_event` log). The row
-- lives at `youtube_publish_settings:singleton` so both reads and the
-- admin UPSERT can target it without an `id` column ambiguity.

DEFINE TABLE IF NOT EXISTS youtube_publish_settings SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS include_credits ON youtube_publish_settings TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS updated_at ON youtube_publish_settings TYPE datetime
    VALUE time::now() DEFAULT time::now();
