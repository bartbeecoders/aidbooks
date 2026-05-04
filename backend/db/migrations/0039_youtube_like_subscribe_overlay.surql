-- Adds the like-and-subscribe overlay toggle to the YouTube publish
-- settings singleton. When true, the publisher burns a
-- "👍 Like & Subscribe!" CTA into the encoded video for two short
-- windows (early in the video and again before the end) via an
-- ffmpeg drawtext filter. Off by default — opt-in.

DEFINE FIELD IF NOT EXISTS like_subscribe_overlay ON youtube_publish_settings
    TYPE bool DEFAULT false;
