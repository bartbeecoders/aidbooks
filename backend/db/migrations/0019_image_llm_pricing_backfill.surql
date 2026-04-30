-- Backfill `cost_per_megapixel` for rows that pre-date 0018.
--
-- `DEFINE FIELD … TYPE float DEFAULT 0.0` only applies the default to new
-- CREATEs, so any `llm` row that existed before migration 0018 keeps a
-- NONE value for the new column. The next read (e.g. the seed's UPSERT
-- MERGE) then trips with:
--   "Found NONE for field `cost_per_megapixel`, … expected a float"
-- This unconditional UPDATE writes 0.0 to every row; new rows already
-- have the default, so re-applying is a no-op.

UPDATE llm SET cost_per_megapixel = 0.0;
