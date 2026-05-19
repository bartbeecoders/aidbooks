-- Allow `mold` as an LLM provider. Mold is a self-hosted local image-gen
-- HTTP server (https://github.com/utensils/mold) that we talk to over
-- `POST /api/generate`. Same per-row routing fields as `openai` (added in
-- 0048): `base_url` points at `mold serve`, `api_key_enc` carries the
-- optional `MOLD_API_KEY`. `model_id` is the mold model slug
-- (e.g. `flux2-klein:q8`, `flux-dev:q4`).
--
-- DEFINE FIELD OVERWRITE replaces the previous ASSERT wholesale; existing
-- rows (open_router / xai / openai) stay valid.

DEFINE FIELD OVERWRITE provider ON llm TYPE string
    ASSERT $value IN ["open_router", "xai", "openai", "mold"];
