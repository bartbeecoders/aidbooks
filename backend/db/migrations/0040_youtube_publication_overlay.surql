-- Per-publication override for the "Like & Subscribe!" overlay.
--   NONE  → fall back to the singleton's `like_subscribe_overlay`.
--   true  → force the overlay on, regardless of the singleton.
--   false → force it off, regardless of the singleton.
-- The publisher resolves this in the dispatch step, so the per-row
-- value is what the encoder actually sees.

DEFINE FIELD IF NOT EXISTS like_subscribe_overlay ON youtube_publication
    TYPE option<bool>;
