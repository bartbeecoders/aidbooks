-- Allow `openai` as an LLM provider, and add the per-row routing columns
-- that drive the OpenAI-compat tab in the admin LLM dialog. Same pattern
-- as 0024 used to widen the `provider` ASSERT for xAI: DEFINE FIELD
-- OVERWRITE replaces the previous definition wholesale; existing rows
-- (all currently `open_router` or `xai`) stay valid.
--
-- `base_url` and `api_key_enc` are optional strings. They're only set on
-- rows with `provider = "openai"`; for OpenRouter/xAI rows they remain
-- NONE. `api_key_enc` is the ChaCha20-Poly1305 ciphertext produced by
-- `youtube::encrypt::encrypt_with_domain(.., LLM_API_KEY_DOMAIN)` — never
-- the plaintext.

DEFINE FIELD OVERWRITE provider ON llm TYPE string
    ASSERT $value IN ["open_router", "xai", "openai"];

DEFINE FIELD IF NOT EXISTS base_url ON llm TYPE option<string>;
DEFINE FIELD IF NOT EXISTS api_key_enc ON llm TYPE option<string>;
