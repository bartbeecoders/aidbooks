-- Songbook snippet count.
--
-- When `is_songbook = true` and `snippet_count > 0`, the create flow
-- enqueues a `SongSnippets` job that resolves the song to a YouTube
-- URL via Tinyfish, downloads the audio with yt-dlp, cuts N
-- evenly-spaced ~12 s clips, and stores them under
-- `<storage>/<audiobook>/snippets/snippet-<i>.wav`. The publish step
-- intercalates them between chapters at upload time.
--
-- 12 is a soft cap that keeps the published video under YouTube's
-- "music-heavy" copyright signal while still giving long books room
-- to breathe.

DEFINE FIELD IF NOT EXISTS snippet_count ON audiobook TYPE int DEFAULT 0
    ASSERT $value >= 0 AND $value <= 12;
