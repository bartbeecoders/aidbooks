"""``flow_chart`` template — sequential boxes connected by arrows.

Param shape:

    { "steps": ["<step 1>", "<step 2>", ..., "<step N>"] }   # 3 ≤ N ≤ 7

Layout heuristics:
  * 3–4 steps  → single horizontal row.
  * 5–7 steps  → two rows, snaking left-to-right then right-to-left so
                 arrows don't have to backtrack across the frame.

Each step reveals as ``box → arrow_into_next_step`` over the same
time slice, so the eye follows the chain without pause.
"""

from __future__ import annotations

from typing import Iterable

import numpy as np
from manim import (
    Arrow,
    Create,
    DOWN,
    FadeIn,
    LEFT,
    ORIGIN,
    RIGHT,
    Rectangle,
    Text,
    UP,
    VGroup,
    Write,
)

from ._base import THEME_ACCENT, THEME_DIM, THEME_FOREGROUND, TemplateScene, phases


# Box geometry — chosen so labels up to ~20 chars fit at 0.36 scale.
BOX_W = 2.2
BOX_H = 0.9


def _layout_positions(n: int) -> list[tuple[float, float]]:
    """Return (x, y) centres for n boxes laid out in 1 or 2 rows.

    1 row when n ≤ 4 (max 4 across 1920px reads cleanly), 2 rows
    otherwise. Snake order means box[i+1] is always adjacent to
    box[i] without a long-range arrow.
    """
    if n <= 4:
        gap = max(BOX_W + 0.6, 8.0 / n)
        x_start = -(n - 1) / 2.0 * gap
        return [(x_start + i * gap, 0.0) for i in range(n)]

    # Two rows. Top row LTR, bottom row RTL — snake.
    top = (n + 1) // 2  # 5→3, 6→3, 7→4
    bottom = n - top
    gap_top = max(BOX_W + 0.6, 7.5 / max(top - 1, 1))
    x_top_start = -(top - 1) / 2.0 * gap_top
    positions = [(x_top_start + i * gap_top, 1.0) for i in range(top)]
    if bottom == 0:
        return positions
    gap_bot = max(BOX_W + 0.6, 7.5 / max(bottom - 1, 1))
    x_bot_end = (bottom - 1) / 2.0 * gap_bot
    # Walk right-to-left so the first bottom box is under (or near)
    # the last top box.
    positions += [(x_bot_end - i * gap_bot, -1.2) for i in range(bottom)]
    return positions


def _arrow_between(p_from: tuple[float, float], p_to: tuple[float, float]) -> Arrow:
    """Build an arrow from one box to the next, sized to the gap.

    The arrow starts/ends at the boxes' edges (not their centres) so
    it sits in the whitespace between them.
    """
    fx, fy = p_from
    tx, ty = p_to
    # Direction
    dx = tx - fx
    dy = ty - fy
    norm = (dx * dx + dy * dy) ** 0.5 or 1.0
    ux, uy = dx / norm, dy / norm
    # Pull each end in by half the box dimension along the dominant axis.
    half = (BOX_W if abs(ux) > abs(uy) else BOX_H) / 2.0 + 0.05
    start = np.array([fx + ux * half, fy + uy * half, 0.0])
    end = np.array([tx - ux * half, ty - uy * half, 0.0])
    return Arrow(
        start=start,
        end=end,
        buff=0,
        color=THEME_ACCENT,
        stroke_width=3,
        max_tip_length_to_length_ratio=0.25,
    )


class FlowChartScene(TemplateScene):
    """See module docstring for params."""

    def construct(self) -> None:
        raw_steps = self.params.get("steps", [])
        if not isinstance(raw_steps, Iterable):
            raw_steps = []
        steps: list[str] = []
        for s in raw_steps:
            if isinstance(s, str) and s.strip():
                steps.append(s.strip())
            if len(steps) >= 7:
                break
        if len(steps) < 2:
            # Degenerate — render the one step we have, no arrows.
            steps = steps or ["(empty flow)"]

        intro, main, outro = phases(self.run_seconds)

        positions = _layout_positions(len(steps))
        boxes: list[VGroup] = []
        for (x, y), label in zip(positions, steps):
            box = Rectangle(
                width=BOX_W,
                height=BOX_H,
                color=THEME_FOREGROUND,
                stroke_width=2.5,
            )
            box.set_fill(THEME_DIM, opacity=0.3)
            box.move_to(np.array([x, y, 0.0]))
            text = Text(label, color=THEME_FOREGROUND).scale(0.32)
            # Shrink to fit if the label is too wide for the box.
            if text.width > box.width * 0.92:
                text.scale(box.width * 0.92 / text.width)
            text.move_to(box.get_center())
            boxes.append(VGroup(box, text))

        arrows = [
            _arrow_between(positions[i], positions[i + 1])
            for i in range(len(positions) - 1)
        ]

        # --- Intro: first box --------------------------------------------
        self.play(FadeIn(boxes[0]), run_time=intro)

        # --- Main: chain reveal ------------------------------------------
        per_step = main / max(1, len(boxes) - 1)
        for i in range(len(boxes) - 1):
            self.play(
                Create(arrows[i]),
                FadeIn(boxes[i + 1]),
                run_time=max(0.4, per_step * 0.95),
            )

        # --- Outro: hold ------------------------------------------------
        self.wait(outro)
