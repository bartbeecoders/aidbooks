-- Pre-publish review.
--
-- Adds two fields to `youtube_publication`:
--   * `review` — when true, the worker stops after encoding the MP4(s) so
--     the user can preview locally before approving the upload.
--   * `preview_ready_at` — set the moment encoding completes in review
--     mode; cleared once the upload phase actually starts.

DEFINE FIELD IF NOT EXISTS review ON youtube_publication TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS preview_ready_at ON youtube_publication
    TYPE option<datetime>;
