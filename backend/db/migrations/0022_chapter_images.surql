-- Extra in-chapter artwork.
--
-- `audiobook.chapter_image_count` controls how many *additional* images
-- (beyond the single `chapter_art_path` cover tile) are generated per
-- chapter. `0` / `NONE` keeps the legacy behaviour.
--
-- Per-chapter image paths are stored on the `chapter` row itself as an
-- ordered array of relative paths, one entry per generated tile. We
-- piggy-back on the chapter row (rather than introducing a new table) so
-- translations naturally get their own slot via the existing
-- per-language chapter rows — though today the generator anchors images
-- to the primary language and translated rows just stay empty.

DEFINE FIELD IF NOT EXISTS chapter_image_count ON audiobook TYPE option<int>
    ASSERT $value = NONE OR ($value >= 0 AND $value <= 10);

DEFINE FIELD IF NOT EXISTS chapter_image_paths ON chapter TYPE option<array<string>>;

-- Make `job.payload` FLEXIBLE so scene-image jobs round-trip their
-- `{ scene_index, total_scenes }` fields. Same trap as 0021 hit on
-- `audiobook.auto_pipeline`: a SCHEMAFULL `option<object>` strips any
-- undeclared inner fields, which would silently make the cover handler
-- treat every per-chapter job as the cover-tile branch.
DEFINE FIELD OVERWRITE payload ON job FLEXIBLE TYPE option<object>;
