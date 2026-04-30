-- Make `audiobook.auto_pipeline` FLEXIBLE.
--
-- The audiobook table is SCHEMAFULL, and migration 0020 declared
-- `auto_pipeline` as `option<object>`. Without FLEXIBLE, SurrealDB
-- strips any inner field that isn't itself declared, so the persisted
-- object came back as an empty `{}` and the chapters handler decoded
-- chapters/cover/audio as `false`, silently skipping narration + chapter
-- art fan-out.
DEFINE FIELD OVERWRITE auto_pipeline ON audiobook FLEXIBLE TYPE option<object>;
