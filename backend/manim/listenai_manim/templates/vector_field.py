"""``vector_field`` template — generic 2D vector field on a grid.

Param shape:

    { "description": "<short phrase>" }

The description string drives a small switch over field shape:

  * "rotational" / "rotation" / "circular" / "curl" → swirl
  * "radial" / "outward" / "source" / "sink" / "gravitational" / "electric" → radial
  * "gradient" / "uniform" / "linear" → straight horizontal
  * (anything else) → swirl, since it's the most visually
    informative when the topic is unknown

We don't try to infer parameters of the field — this template's job
is to be *visually evocative*, not numerically accurate. The
description is rendered as a caption so the narrator's text carries
the precision and the diagram carries the intuition.
"""

from __future__ import annotations

import numpy as np
from manim import (
    Arrow,
    DOWN,
    FadeIn,
    FadeOut,
    LEFT,
    ORIGIN,
    RIGHT,
    Text,
    UP,
    VGroup,
)

from ._base import THEME_ACCENT, THEME_DIM, THEME_FOREGROUND, TemplateScene, phases


def _classify(description: str) -> str:
    s = description.lower()
    if any(k in s for k in ("rotat", "circul", "curl", "swirl", "spin")):
        return "rotational"
    if any(k in s for k in ("radial", "outward", "inward", "source", "sink", "gravit", "electric", "field around")):
        return "radial"
    if any(k in s for k in ("gradient", "uniform", "constant", "linear")):
        return "uniform"
    return "rotational"  # most visually rich default


def _vector_at(kind: str, x: float, y: float) -> tuple[float, float]:
    """Return the (dx, dy) the field points to at (x, y)."""
    if kind == "rotational":
        return -y, x
    if kind == "radial":
        r = max(0.001, np.sqrt(x * x + y * y))
        return x / r, y / r
    # uniform
    return 1.0, 0.0


class VectorFieldScene(TemplateScene):
    """See module docstring for params."""

    def construct(self) -> None:
        description = str(self.params.get("description", "")).strip()
        kind = _classify(description)

        intro, main, outro = phases(self.run_seconds)

        # 7 × 5 grid centred on origin. Spacing 1.0 keeps the field
        # legible without crowding the frame.
        xs = np.linspace(-3, 3, 7)
        ys = np.linspace(-2, 2, 5)
        max_norm = 1.4  # cap arrow magnitude so a divergent field
                        # (e.g. pure radial near origin) doesn't
                        # produce one giant arrow.

        arrows = VGroup()
        for x in xs:
            for y in ys:
                dx, dy = _vector_at(kind, float(x), float(y))
                # Scale + clip
                norm = (dx * dx + dy * dy) ** 0.5
                if norm == 0:
                    continue
                scale = min(0.55, 0.55 * (norm / max(norm, max_norm)))
                tail = np.array([float(x), float(y), 0.0])
                head = tail + scale * np.array([dx / norm, dy / norm, 0.0])
                arrow = Arrow(
                    start=tail,
                    end=head,
                    buff=0,
                    stroke_width=2.5,
                    color=THEME_DIM,
                    max_tip_length_to_length_ratio=0.35,
                )
                arrows.add(arrow)

        # Caption — show whatever the LLM gave us, since the field
        # shape alone doesn't disambiguate "gravitational" from
        # "electric monopole" etc. Cap length so it fits the frame.
        caption_text = description if description else "vector field"
        if len(caption_text) > 70:
            caption_text = caption_text[:67].rstrip() + "…"
        caption = Text(caption_text, color=THEME_FOREGROUND).scale(0.42)
        caption.to_edge(DOWN, buff=0.4)

        # Reveal the field as a single FadeIn — drawing each arrow
        # individually would steal the whole `main` budget on a 35-
        # arrow grid. The Pre-G.4 design discussed staggered reveal;
        # in practice it looked busy. One clean fade reads better.
        self.play(FadeIn(arrows, lag_ratio=0.02), run_time=main * 0.7)
        self.play(FadeIn(caption), run_time=main * 0.3)

        self.wait(max(0.0, outro - 0.3))
        # No tearing-down fade — the next scene's intro will cut hard
        # via concat. (Phase G.6 will handle inter-segment crossfades
        # if we want them.)
        self.play(FadeOut(arrows), FadeOut(caption), run_time=0.3)
