"""``axes_with_curve`` template — qualitative curve on labelled axes.

For paragraphs that describe *the shape* of a relationship without a
precise formula. Five curve kinds cover most cases:

  * ``linear``      — y = x  (proportionality)
  * ``exp``         — y = e^x  (exponential growth)
  * ``log``         — y = log(x)  (diminishing returns)
  * ``sin`` / ``cos`` — periodic / oscillatory

Param shape:

    {
      "x_label": "...", "y_label": "...",
      "curve_kind": "linear|exp|log|sin|cos",
      "emphasize": "<optional callout>"
    }

No numeric ticks — the axes show a labelled X / Y arrow only, so the
viewer reads the *shape* rather than reading off values.
"""

from __future__ import annotations

from typing import Callable

import numpy as np
from manim import (
    Axes,
    Create,
    FadeIn,
    FadeOut,
    Text,
    UP,
    UR,
    Write,
)

from ._base import THEME_ACCENT, THEME_DIM, THEME_FOREGROUND, TemplateScene, phases


# Curve catalogue. Each entry is `(callable, default x range)`. The
# ranges are chosen so the curve fills the visible band cleanly
# without clipping.
_CURVES: dict[str, tuple[Callable[[float], float], tuple[float, float]]] = {
    "linear": (lambda x: x, (-3.0, 3.0)),
    "exp": (lambda x: np.exp(x * 0.6) - 1.0, (-3.0, 3.0)),
    "log": (lambda x: np.log(x + 0.1) if x > -0.099 else float("nan"), (0.1, 5.0)),
    "sin": (lambda x: np.sin(x), (-np.pi, np.pi)),
    "cos": (lambda x: np.cos(x), (-np.pi, np.pi)),
}


class AxesWithCurveScene(TemplateScene):
    """See module docstring for params."""

    def construct(self) -> None:
        x_label = str(self.params.get("x_label", "x")).strip() or "x"
        y_label = str(self.params.get("y_label", "y")).strip() or "y"
        kind = str(self.params.get("curve_kind", "linear")).strip().lower()
        emphasize = self.params.get("emphasize")
        if emphasize is not None:
            emphasize = str(emphasize).strip() or None

        if kind not in _CURVES:
            kind = "linear"
        f, (x_min, x_max) = _CURVES[kind]

        # Sample the curve to find a sensible y range. Same trick as
        # function_plot: drop NaN so log doesn't blow up the auto-fit.
        ys = []
        for x in np.linspace(x_min, x_max, 100):
            try:
                y = float(f(x))
                if np.isfinite(y):
                    ys.append(y)
            except Exception:
                continue
        if ys:
            y_min, y_max = min(ys), max(ys)
            pad = max(0.3, 0.15 * abs(y_max - y_min))
            y_min -= pad
            y_max += pad
        else:
            y_min, y_max = -2.0, 2.0

        intro, main, outro = phases(self.run_seconds)

        axes = Axes(
            x_range=[x_min, x_max, max(1.0, (x_max - x_min) / 4.0)],
            y_range=[y_min, y_max, max(1.0, (y_max - y_min) / 4.0)],
            tips=True,  # tips read better when there's no numeric scale
            axis_config={
                "color": THEME_DIM,
                "include_numbers": False,
                "include_ticks": False,
            },
        )
        x_text = Text(x_label, color=THEME_FOREGROUND).scale(0.4)
        x_text.next_to(axes.x_axis.get_end(), 0.6 * UP)
        y_text = Text(y_label, color=THEME_FOREGROUND).scale(0.4)
        y_text.next_to(axes.y_axis.get_end(), 0.6 * UP)

        try:
            graph = axes.plot(f, x_range=[x_min, x_max], color=THEME_ACCENT)
        except Exception:
            graph = None

        # Reveal: axes + labels first, then curve.
        self.play(Create(axes), run_time=intro * 0.6)
        self.play(Write(x_text), Write(y_text), run_time=intro * 0.4)

        if graph is not None:
            self.play(Create(graph), run_time=main * 0.85)
        else:
            self.wait(main * 0.85)

        if emphasize:
            tag = Text(f"Note: {emphasize}", color=THEME_ACCENT).scale(0.45)
            tag.to_corner(UR, buff=0.4)
            self.play(FadeIn(tag), run_time=main * 0.15)
        else:
            self.wait(main * 0.15)

        self.wait(max(0.0, outro - 0.3))
        if emphasize:
            self.play(FadeOut(tag), run_time=0.3)
