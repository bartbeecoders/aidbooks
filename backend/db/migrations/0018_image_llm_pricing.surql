-- Image-LLM pricing.
--
-- Image generation models (e.g. `function = "image"`) don't price by token,
-- so the existing `cost_prompt_per_1k` / `cost_completion_per_1k` fields
-- are meaningless for them. Add a `cost_per_megapixel` column instead;
-- text models leave it at 0.

DEFINE FIELD IF NOT EXISTS cost_per_megapixel ON llm TYPE float DEFAULT 0.0;
