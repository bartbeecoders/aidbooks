-- Allow `xai` as an LLM provider. The original 0001_init constraint
-- whitelisted only `open_router`; with the xAI tab in the admin LLM
-- dialog (and `LlmProvider::Xai` in the domain enum) we now need rows
-- whose `provider = "xai"`.
--
-- DEFINE FIELD OVERWRITE replaces the existing definition wholesale —
-- same pattern 0023 uses to extend `job.kind`. Existing rows keep their
-- value (all currently `open_router`) and stay valid against the new
-- whitelist.

DEFINE FIELD OVERWRITE provider ON llm TYPE string
    ASSERT $value IN ["open_router", "xai"];
