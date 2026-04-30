-- Per-language footer appended to every YouTube description on publish.
--
-- Singleton-per-language: the language code (BCP-47) is the record id, so
-- the publisher can look up `youtube_description_footer:`<lang>`` directly.
-- An entry's `text` is appended verbatim after a blank line at the end of
-- the description; admins use it for disclaimers + a website backlink.

DEFINE TABLE IF NOT EXISTS youtube_description_footer SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS text ON youtube_description_footer TYPE string
    ASSERT string::len($value) <= 4000;
DEFINE FIELD IF NOT EXISTS updated_at ON youtube_description_footer TYPE datetime
    DEFAULT time::now();
