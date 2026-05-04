"""``equation_steps`` template — chain of equation transformations.

Param shape:

    { "steps": ["F = m \\cdot a", "a = F / m", ...] }   # 2 ≤ N ≤ 6

Each step is a `MathTex`. Between consecutive steps we try
``TransformMatchingShapes`` (smooth shape morph for visually
similar expressions) and fall back to ``FadeOut + FadeIn`` if the
transform raises (e.g. wildly different LaTeX trees).

Bad LaTeX in a step crashes Manim's `MathTex` constructor, so each
step is rendered defensively — a step that fails to compile becomes
a Pango ``Text`` showing the literal source. The chapter still
makes its point, the narrator covers the missing math.
"""

from __future__ import annotations

from typing import Iterable

from manim import (
    DOWN,
    FadeIn,
    FadeOut,
    MathTex,
    Mobject,
    Text,
    Transform,
    TransformMatchingShapes,
    UP,
    Write,
)

from ._base import THEME_ACCENT, THEME_FOREGROUND, TemplateScene, phases


def _build_step(latex: str) -> Mobject:
    """Render one step. Returns a MathTex on success or a Text on
    LaTeX failure — never raises, never returns None.
    """
    try:
        return MathTex(latex, color=THEME_FOREGROUND)
    except Exception:
        # LaTeX compilation failed (typo, missing macro, etc.).
        # Surface the raw source so the chapter has a placeholder.
        return Text(latex, color=THEME_FOREGROUND).scale(0.5)


class EquationStepsScene(TemplateScene):
    """See module docstring for params."""

    def construct(self) -> None:
        raw_steps = self.params.get("steps", [])
        if not isinstance(raw_steps, Iterable):
            raw_steps = []
        steps: list[str] = []
        for s in raw_steps:
            if isinstance(s, str) and s.strip():
                steps.append(s.strip())
            if len(steps) >= 6:
                break
        if not steps:
            steps = ["?"]

        intro, main, outro = phases(self.run_seconds)

        # First step
        current = _build_step(steps[0]).scale(1.2)
        current.move_to([0, 0, 0])
        self.play(Write(current), run_time=intro)

        # Subsequent steps — transform from the current to the next.
        per_step = main / max(1, len(steps) - 1)
        for i in range(1, len(steps)):
            nxt = _build_step(steps[i]).scale(1.2)
            nxt.move_to([0, 0, 0])
            try:
                # TransformMatchingShapes works best when both ends
                # are MathTex of similar structure. It silently does
                # nothing useful when the shapes don't match — we
                # detect failure via exception, not behaviour.
                self.play(
                    TransformMatchingShapes(current, nxt),
                    run_time=max(0.5, per_step * 0.7),
                )
            except Exception:
                self.play(
                    FadeOut(current),
                    run_time=max(0.25, per_step * 0.35),
                )
                self.play(
                    FadeIn(nxt),
                    run_time=max(0.25, per_step * 0.35),
                )
            self.wait(per_step * 0.3)
            current = nxt

        self.wait(outro)
