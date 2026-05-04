"""``function_plot`` template — animated y = f(x) over a domain.

Param shape (from `paragraph_visual_v1.md`):

    {
      "fn": "<expression in x>",
      "domain": [<min>, <max>],
      "emphasize": "<optional callout>"
    }

Examples that work today:
  * ``{"fn": "x**2 - 2*x + 1", "domain": [-3, 3], "emphasize": "vertex"}``
  * ``{"fn": "1 / (1 + exp(-x))", "domain": [-6, 6], "emphasize": "saturation"}``
  * ``{"fn": "sin(x)", "domain": [-pi, pi]}``  (if pi is in the params,
    we don't parse it; pass numeric).

Hardening:
  * ``fn`` is evaluated in a tiny sandbox: only numpy math + a few
    constants are reachable. ``__builtins__`` is empty, so e.g.
    ``open(...)`` or ``import`` fail outright.
  * If eval / plot raises (bad fn, NaN-only output, …) the scene
    falls back to rendering the literal expression as MathTex over
    blank axes — the chapter still has *something* to look at.
"""

from __future__ import annotations

from typing import Callable

import numpy as np
from manim import (
    Axes,
    Create,
    FadeIn,
    FadeOut,
    MathTex,
    Text,
    UP,
    UR,
    Write,
)

from ._base import (
    THEME_ACCENT,
    THEME_DIM,
    THEME_FOREGROUND,
    TemplateScene,
    phases,
)


# Sandboxed namespace for `eval`. Only deterministic, side-effect-free
# math. Adding a new function here is a deliberate choice — the LLM's
# prompt enumerates which expressions it should produce, and broader
# eval surface = bigger hallucination footprint.
_SAFE_NAMES: dict = {
    "__builtins__": {},
    "sin": np.sin,
    "cos": np.cos,
    "tan": np.tan,
    "asin": np.arcsin,
    "acos": np.arccos,
    "atan": np.arctan,
    "exp": np.exp,
    "log": np.log,
    "log2": np.log2,
    "log10": np.log10,
    "sqrt": np.sqrt,
    "abs": np.abs,
    "pi": float(np.pi),
    "e": float(np.e),
}


def _compile_fn(expr: str) -> Callable[[float], float]:
    """Return a ``f(x) -> y`` callable from a string expression.

    Raises ``ValueError`` if the expression doesn't compile or
    references unsafe names. Doesn't catch eval-time errors — those
    are caught when the scene actually plots and falls back to a
    text-only display.
    """
    code = compile(expr, "<function_plot>", "eval")
    # Cheap structural check: any name that isn't in the safe ns
    # blows up here rather than at every plot point.
    for name in code.co_names:
        if name not in _SAFE_NAMES:
            raise ValueError(f"unsafe identifier in fn: {name!r}")

    def f(x: float) -> float:
        return eval(code, _SAFE_NAMES, {"x": x})  # noqa: S307

    return f


def _y_range_for(f: Callable[[float], float], x_min: float, x_max: float) -> tuple[float, float]:
    """Sample the function 200 times across the domain, return a
    padded ``(y_min, y_max)`` for the axes. Drops NaN/inf so a
    badly-behaved function doesn't blow up the auto-fit.
    """
    xs = np.linspace(x_min, x_max, 200)
    ys = []
    for x in xs:
        try:
            y = float(f(x))
            if np.isfinite(y):
                ys.append(y)
        except Exception:
            continue
    if not ys:
        return -3.0, 3.0
    y_min, y_max = float(min(ys)), float(max(ys))
    pad = max(0.5, 0.1 * abs(y_max - y_min))
    return y_min - pad, y_max + pad


class FunctionPlotScene(TemplateScene):
    """Animated plot of y = f(x). See module docstring for params."""

    def construct(self) -> None:
        fn_str = str(self.params.get("fn", "x")).strip() or "x"
        domain = self.params.get("domain", [-3, 3])
        try:
            x_min, x_max = float(domain[0]), float(domain[1])
            if x_min >= x_max:
                x_min, x_max = -3.0, 3.0
        except (TypeError, ValueError, IndexError):
            x_min, x_max = -3.0, 3.0
        emphasize = self.params.get("emphasize")
        if emphasize is not None:
            emphasize = str(emphasize).strip() or None

        intro, main, outro = phases(self.run_seconds)

        # Try to compile the function. On failure, render the
        # expression as text over empty axes — never crash on bad
        # LLM output.
        try:
            f = _compile_fn(fn_str)
            y_min, y_max = _y_range_for(f, x_min, x_max)
            ok = True
        except Exception:
            f = lambda _x: 0.0  # noqa: E731
            y_min, y_max = -3.0, 3.0
            ok = False

        axes = Axes(
            x_range=[x_min, x_max, max(1.0, (x_max - x_min) / 5.0)],
            y_range=[y_min, y_max, max(1.0, (y_max - y_min) / 4.0)],
            tips=False,
            axis_config={"color": THEME_DIM},
        )
        # Manim's MathTex needs LaTeX-friendly syntax. The LLM emits
        # Python-ish (e.g. ``x**2``); a small substitution keeps the
        # label readable without a full transpiler.
        latex_label = (
            fn_str.replace("**", "^").replace("*", "\\cdot ")
        )

        try:
            label = MathTex(rf"f(x) = {latex_label}", color=THEME_FOREGROUND)
        except Exception:
            label = Text(f"f(x) = {fn_str}", color=THEME_FOREGROUND).scale(0.6)
        label.to_edge(UP, buff=0.4)

        # --- Intro: axes appear -------------------------------------------
        self.play(Create(axes), run_time=intro * 0.7)
        self.play(FadeIn(label), run_time=intro * 0.3)

        # --- Main: graph reveal -------------------------------------------
        if ok:
            try:
                graph = axes.plot(f, x_range=[x_min, x_max], color=THEME_ACCENT)
                self.play(Create(graph), run_time=main * 0.85)
            except Exception:
                # Plot failed (e.g. fn returns NaN over the domain).
                self.wait(main * 0.85)
        else:
            # Compile failed — keep the label visible, hold for the
            # main slot so total runtime still matches the planner.
            self.wait(main * 0.85)

        # --- Optional emphasis callout ------------------------------------
        if emphasize:
            tag = Text(f"Note: {emphasize}", color=THEME_ACCENT).scale(0.45)
            tag.to_corner(UR, buff=0.4)
            self.play(FadeIn(tag), run_time=main * 0.15)
        else:
            self.wait(main * 0.15)

        # --- Outro: hold then fade ----------------------------------------
        self.wait(max(0.0, outro - 0.3))
        if emphasize:
            self.play(FadeOut(tag), run_time=0.3)
