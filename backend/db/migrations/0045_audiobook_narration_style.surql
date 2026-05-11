-- Narration style / mood / voice-preset settings.
--
-- Three orthogonal knobs that shape both the LLM prose generation and
-- (via the speech-tag palette) the TTS delivery:
--
--   narration_style    — single style hint that biases plot + tone
--                        (`drama`, `humor`, `sketch`, `erotic`,
--                        `child_friendly`, `educational`, `natural`).
--                        `natural` (or NONE) keeps the genre-driven
--                        default the chapter LLM picks on its own.
--
--   narration_intensity — additive emotional dial as an array of
--                         tags the user can combine
--                         (`intense`, `dramatic`, `emotional`,
--                         `expressive`). Empty = neutral delivery.
--                         Stored as array so a thriller can be both
--                         `intense` and `dramatic`.
--
--   voice_preset       — shorthand the UI uses to pre-fill
--                        `voice_roles` (e.g. `single_male`,
--                        `duo_female`, `mixed`). Persisted so the
--                        detail page can re-render the same picker
--                        layout the user picked at create time.
--
-- All three are optional and default-safe — legacy rows keep narrating
-- exactly as before. The chapter + outline prompts only branch on them
-- when they're set.

DEFINE FIELD IF NOT EXISTS narration_style ON audiobook
    TYPE option<string>;
DEFINE FIELD IF NOT EXISTS narration_intensity ON audiobook
    TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS voice_preset ON audiobook
    TYPE option<string>;
