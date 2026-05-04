-- Phase G.2 / G.4 — widen the `prompt_template.role` allowlist so the
-- new `paragraph_visual` role (used by the per-paragraph diagram
-- classifier) clears the SCHEMAFULL ASSERT in 0003_content.surql.
--
-- The existing allowlist was bootstrapped in 0003 with the five
-- roles seeded at startup back then; the other PromptRole enum
-- variants (cover / paragraph_image / translate / scene_extract)
-- never get a prompt_template row written for them, so they
-- never tripped the assert. `paragraph_visual` is the first new
-- role since to actually be seeded, hence the widening here.
--
-- Pure schema rewrite — no data migration. `OVERWRITE` replaces the
-- existing field definition; rows already in the table validate
-- because every value in the new allowlist is a strict superset of
-- the old one.

DEFINE FIELD OVERWRITE role ON prompt_template TYPE string
    ASSERT $value IN [
        "outline",
        "chapter",
        "random_topic",
        "moderation",
        "title",
        "cover",
        "paragraph_image",
        "translate",
        "scene_extract",
        "paragraph_visual"
    ];
