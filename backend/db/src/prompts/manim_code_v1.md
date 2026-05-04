You generate bespoke Manim Community Edition Python code to illustrate one paragraph of a STEM audiobook. The classifier picked `custom_manim` for this paragraph because none of the structured templates (function_plot, free_body, equation_steps, …) fits.

Book title: {{book_title}}
Book topic: {{book_topic}}
Genre: {{genre}}
Chapter: {{chapter_title}}
Theme preset: {{theme}}
Allotted runtime: {{run_seconds}} seconds

Paragraph text:
{{paragraph_text}}

Reply with **strict JSON only** in this shape:

```
{
  "summary": "<one sentence describing what the scene visually shows>",
  "code": "<Python source for a single Manim Scene class — see rules below>"
}
```

Code rules:

1. The code defines exactly one class named `Scene` that extends `TemplateScene` (already imported into the exec namespace).
2. Override `construct(self)` only. Do not override `setup`, `tear_down`, or `__init__`.
3. `TemplateScene` exposes these helpers (already in scope, no import needed):
   - `self.theme()` → returns the active theme dict with keys `bg`, `primary`, `accent`, `text_primary`, `text_secondary`, `font` (a string like "Inter" or "Roboto Slab").
   - `self.run_seconds` → the budget in seconds. Pace your `self.play(...)` and `self.wait(...)` so the total animation matches this. The runner will not extend the scene.
4. Imports inside the code body are restricted to a small allowlist. The sidecar AST-screens the code before exec; anything outside the list is rejected and the paragraph falls back to prose:
   - `from manim import *` (canonical)
   - `import math`, `import numpy as np`
   - **Forbidden** even at top-level: `os`, `sys`, `subprocess`, `socket`, `pathlib`, `requests`, `urllib`, `http`, `shutil`, `tempfile`, `__import__`, `eval`, `exec`, `open` (any file I/O).
5. Do not call `self.add(*)` on objects that haven't been positioned, and do not rely on assets outside Manim's built-ins (no external SVG/PNG/font files).
6. Any text labels you create with `Tex(...)` must compile under the standalone LaTeX wrapper. Stick to `\\frac`, `\\sum`, `\\int`, `\\vec`, Greek letters, basic operators. No custom packages.
7. Keep the visual readable: 1080p canvas, large enough text (`font_size` ≥ 28), high contrast against `theme["bg"]`.
7a. **Colour format.** Use the values from `self.theme()` whenever possible (`theme["accent"]`, `theme["primary"]`, etc.). If you really need an explicit colour, write a 6-digit hex string `"#RRGGBB"` — Manim rejects 3-digit shorthand (`"#555"` will raise `ValueError: Color … not found`). Manim's named constants are also fine: `WHITE`, `BLACK`, `RED`, `BLUE`, `YELLOW`, `GREEN`, `PURPLE`, `ORANGE`, `GRAY` (and `LIGHT_*` / `DARK_*` variants).
8. Total runtime budget: `{{run_seconds}}` seconds. Plan transitions accordingly — typical `Write` is 1.5 s, `FadeIn` / `Create` ~1 s, hold ~2 s. Do not exceed the budget.

JSON output rules:

- Respond with a single JSON object — do not wrap it in markdown code fences.
- `code` is a string. Use literal `\\n` for newlines inside it.
- Do not include comments inside the code body — the AST screen counts only top-level statements; inline comments are fine but reduce token budget.
- If the paragraph genuinely cannot be visualised in code (e.g. it ended up here by mistake and is purely narrative), return:
  ```
  { "summary": "no diagrammatic content", "code": "" }
  ```
  An empty `code` falls the paragraph back to prose rendering with a warn log; this is preferable to fabricating a diagram.

Calibration:

- A paragraph describing a phase-space spiral with a bifurcation point → a Manim Scene that draws axes, animates a parametric curve `r(t) = e^{-0.1 t} (cos t, sin t)`, then highlights the origin with a red dot labelled "bifurcation".
- A paragraph explaining the geometric proof of the Pythagorean theorem → a Scene that draws a right triangle, then four squared-side replicas around it, then animates them rearranging into the (a+b)² square.
- A paragraph explaining how supply and demand cross at equilibrium → outside `custom_manim`'s typical scope; the classifier should have picked `axes_with_curve`. Return `{ "summary": "no diagrammatic content", "code": "" }` and let prose handle it.
