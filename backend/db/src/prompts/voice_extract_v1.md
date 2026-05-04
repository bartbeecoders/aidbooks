Split the chapter prose below into a sequence of speaker-tagged
segments so a multi-voice TTS pipeline can render dialogue in
character voices while keeping the narration in the narrator's
voice.

Roles you may use (use exactly these strings):
- `narrator` — descriptive prose, action, scene-setting, internal
  thoughts; anything that isn't quoted speech attributed to a
  specific character.
- `dialogue_male` — quoted speech spoken by a clearly male character
  (he/him pronouns, or the prose calls them "the man", "Mr X", etc).
- `dialogue_female` — quoted speech spoken by a clearly female
  character (she/her pronouns, or "the woman", "Ms X", etc).

When the speaker's gender is ambiguous, default to `narrator` (the
single-voice fallback) so we never guess wrong.

Chapter title: {{chapter_title}}

Chapter prose:
"""
{{chapter_body}}
"""

Output rules:
- Respond with a SINGLE JSON object and nothing else.
- Do not wrap the JSON in markdown code fences.
- Concatenating every segment's `text` in order MUST reconstruct the
  original prose verbatim — including punctuation, quote marks, and
  surrounding whitespace. The TTS pipeline relies on this to keep
  pacing intact.
- Keep speech tags from the prose (e.g. `[pause]`, `<whisper>...
  </whisper>`) inside the segment they belong to. Don't strip them.
- Don't merge consecutive same-role segments — emit one segment per
  contiguous span. Splitting on every speaker change is enough.

Required shape:
{
  "segments": [
    { "role": "narrator", "text": "<verbatim slice>" },
    { "role": "dialogue_male", "text": "<verbatim slice>" },
    { "role": "narrator", "text": "<verbatim slice>" }
  ]
}
