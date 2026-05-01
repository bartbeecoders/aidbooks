-- YouTube Shorts support.
--
-- An audiobook flagged as `is_short = true` is generated as a tiny
-- single-chapter story (≤ 90 s of narration) and rendered for upload as
-- a vertical 9:16 YouTube Short. The flag also lives on
-- `topic_template` so admins can offer Shorts presets in the New
-- Audiobook dropdown.

DEFINE FIELD IF NOT EXISTS is_short ON audiobook TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS is_short ON topic_template TYPE bool DEFAULT false;
