You label paragraphs from a STEM audiobook chapter with the diagram template that should illustrate them. Reply with strict JSON only.

Book title: {{book_title}}
Book topic: {{book_topic}}
Genre: {{genre}}
Chapter: {{chapter_title}}

Paragraphs:
{{paragraph_listing}}

For each paragraph that has a clear, concrete diagrammatic representation, return one entry. Skip paragraphs that are purely narrative, motivational, or transitional — those will be rendered as prose. Only label paragraphs where the visual genuinely *adds understanding* the words alone don't carry.

Allowed `visual_kind` values + the `visual_params` shape each one expects:

- `function_plot` — a single function plotted on standard axes.
  `{ "fn": "<expression in x, e.g. x^2 - 2*x + 1>", "domain": [<min>, <max>], "emphasize": "<optional, e.g. 'vertex' or 'roots'>" }`

- `axes_with_curve` — a labelled relationship without an exact formula.
  `{ "x_label": "...", "y_label": "...", "curve_kind": "linear|exp|log|sin|cos", "emphasize": "<optional>" }`

- `vector_field` — a 2D vector field on a grid.
  `{ "description": "<short phrase, e.g. 'gravitational field around a point mass'>" }`

- `free_body` — a free-body diagram.
  `{ "object": "<noun phrase, e.g. 'block on incline'>", "forces": ["gravity", "normal", "friction", ...] }`

- `flow_chart` — a sequential process.
  `{ "steps": ["<step 1>", "<step 2>", ...] }`  (3–7 steps)

- `bar_chart` — a small categorical comparison.
  `{ "data": [{"label": "...", "value": <number>}, ...] }`  (2–8 bars)

- `equation_steps` — a chain of math transformations.
  `{ "steps": ["<LaTeX or plain math, step 1>", ...] }`  (2–6 steps)

- `neural_net_layer` — a feedforward network sketch.
  `{ "neurons": [<input>, <hidden 1>, <hidden 2>, ..., <output>] }`  (2–5 layers)

- `custom_manim` — escape hatch for paragraphs whose visual is genuinely diagrammatic but doesn't fit any template above (phase portraits, geometric proofs, animations of an algorithm, etc.). Use **sparingly** — at most 2 per chapter, and only when no structured template would do the idea justice. A separate code-generation LLM will write bespoke Manim code for it.
  `{ "rationale": "<one sentence: why no other template fits>" }`

Output rules:
- Respond with a SINGLE JSON object: `{ "visuals": [{ "index": <int>, "visual_kind": "<one of the above>", "visual_params": <object matching the kind> }, ...] }`.
- `index` must match a bracketed number from the paragraph listing above.
- Do NOT wrap the JSON in markdown code fences.
- Do NOT invent indices that aren't listed.
- Do NOT use a `visual_kind` outside the allowed list. If a paragraph doesn't fit any template cleanly, omit it from the response (it'll be rendered as prose).
- It's fine to return zero visuals if the chapter is genuinely all narrative — over-labelling is worse than under-labelling, since each label triggers a render.
- Cap the response at 8 visuals across the chapter. Pick the ones whose diagrams best teach the concept.

Calibration:
- "Newton's second law: F = ma. The acceleration depends linearly on the applied force." → `equation_steps` with `["F = m \\cdot a", "a = F / m"]`.
- "When a block rests on an incline, three forces act on it: gravity, normal force, and friction." → `free_body` with `{"object": "block on incline", "forces": ["gravity", "normal", "friction"]}`.
- "Trust is the substrate of every transaction." → omit (narrative, no diagram).
- "Consider a sigmoid: σ(x) = 1 / (1 + e^{-x}). It saturates at the extremes." → `function_plot` with `{"fn": "1 / (1 + exp(-x))", "domain": [-6, 6], "emphasize": "saturation"}`.
