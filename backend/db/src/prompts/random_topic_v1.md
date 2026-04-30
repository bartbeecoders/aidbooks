Invent a fresh, specific audiobook topic suitable for a 30-90 minute listen.

Optional seed or theme hint: {{seed}}
Output language: {{language}}

Output rules:
- Respond with a SINGLE JSON object and nothing else.
- Do not wrap the JSON in markdown code fences.

Required shape:
{
  "topic": "<specific, evocative topic — one sentence, 8 to 25 words>",
  "genre": "<one of: educational, narrative, conversational, mystery, sci_fi, history, biography, how_to>",
  "length": "<one of: short, medium, long>"
}

Guidelines:
- Write the `topic` field in the requested output language. Keep the
  `genre` and `length` values in the canonical English keys above so the
  app can route them.
- Prefer concrete, specific topics over broad genres (e.g. "the 1848 cholera outbreak in London" over "diseases").
- Balance education vs. narrative — about 60% narrative, 40% educational.
- Avoid celebrity gossip, real crimes involving named individuals, or hateful framings.
