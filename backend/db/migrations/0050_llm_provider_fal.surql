-- Allow `fal` as an LLM provider. fal.ai is a hosted image-gen platform
-- (https://fal.ai/docs/documentation); each row points at the sync REST
-- host via `base_url` (defaults to https://fal.run when left blank) and
-- stores the FAL_KEY in `api_key_enc`. `model_id` is the fal slug
-- (e.g. `fal-ai/flux/dev`, `fal-ai/flux/schnell`, `fal-ai/ideogram/v3`).
--
-- DEFINE FIELD OVERWRITE replaces the previous ASSERT wholesale; existing
-- rows (open_router / xai / openai / mold) stay valid.

DEFINE FIELD OVERWRITE provider ON llm TYPE string
    ASSERT $value IN ["open_router", "xai", "openai", "mold", "fal"];
