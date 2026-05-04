"""``neural_net_layer`` template — feedforward network sketch.

Param shape:

    { "neurons": [<input>, <hidden 1>, ..., <output>] }   # 2 ≤ layers ≤ 5

Each integer is the neuron count for one layer (input first, output
last). We render columns of ``Circle`` mobjects with full-mesh lines
between adjacent layers, then animate layer-by-layer.

For very dense layers we cap the visible neuron count and add an
ellipsis dot to keep the visual legible. The narrator's text carries
the precise count.
"""

from __future__ import annotations

from typing import Iterable

import numpy as np
from manim import (
    Circle,
    Create,
    DOWN,
    FadeIn,
    LEFT,
    Line,
    ORIGIN,
    RIGHT,
    Text,
    UP,
    VGroup,
)

from ._base import THEME_ACCENT, THEME_DIM, THEME_FOREGROUND, TemplateScene, phases


# Visual caps — anything above MAX_VISIBLE_NEURONS gets truncated to
# `[first ... ellipsis ... last]` rendered with three real circles
# and a `…` text spacer. Keeps a 1024-wide layer from blowing up the
# frame.
MAX_LAYERS = 5
MAX_VISIBLE_NEURONS = 7
NEURON_RADIUS = 0.18
NEURON_GAP = 0.55          # vertical spacing between neurons
LAYER_GAP = 2.4            # horizontal spacing between columns


def _layer_circles(count: int) -> tuple[VGroup, list[np.ndarray]]:
    """Build a vertical column of ``count`` circles, return (group,
    centres). When count > MAX_VISIBLE_NEURONS, draws first 3 +
    ellipsis + last 2 instead so the column doesn't run off-screen.
    """
    if count <= 0:
        return VGroup(), []

    visible_count = min(count, MAX_VISIBLE_NEURONS)
    truncated = count > MAX_VISIBLE_NEURONS
    centres: list[np.ndarray] = []
    group = VGroup()

    for i in range(visible_count):
        # Centre vertically around y = 0
        y = (visible_count - 1) / 2.0 - i
        y *= NEURON_GAP
        c = Circle(
            radius=NEURON_RADIUS,
            color=THEME_FOREGROUND,
            stroke_width=2.0,
        )
        c.set_fill(THEME_DIM, opacity=0.6)
        centre = np.array([0.0, y, 0.0])
        c.move_to(centre)
        centres.append(centre)
        group.add(c)

    if truncated:
        # Stick a "…" text in the middle to suggest the gap.
        ellipsis = Text("…", color=THEME_FOREGROUND).scale(0.5)
        ellipsis.move_to([0.5, 0, 0])  # offset so it doesn't overlap circles
        group.add(ellipsis)

    return group, centres


def _connect(a_centres: Iterable[np.ndarray], b_centres: Iterable[np.ndarray]) -> VGroup:
    """Full-mesh lines from each centre in `a` to each in `b`. Lines
    sit behind the circles (they're added to the group first, so
    later additions render on top).
    """
    lines = VGroup()
    for ac in a_centres:
        for bc in b_centres:
            line = Line(
                start=ac,
                end=bc,
                stroke_width=1.0,
                color=THEME_DIM,
            )
            lines.add(line)
    return lines


class NeuralNetLayerScene(TemplateScene):
    """See module docstring for params."""

    def construct(self) -> None:
        raw_neurons = self.params.get("neurons", [])
        if not isinstance(raw_neurons, Iterable):
            raw_neurons = []
        neurons: list[int] = []
        for n in raw_neurons:
            try:
                v = int(n)
            except (TypeError, ValueError):
                continue
            if v >= 1:
                neurons.append(v)
            if len(neurons) >= MAX_LAYERS:
                break
        if len(neurons) < 2:
            self._render_placeholder()
            return

        intro, main, outro = phases(self.run_seconds)

        # Build columns + position them along x.
        columns: list[VGroup] = []
        col_centres: list[list[np.ndarray]] = []
        n_cols = len(neurons)
        x_start = -(n_cols - 1) / 2.0 * LAYER_GAP
        for i, count in enumerate(neurons):
            col, centres = _layer_circles(count)
            x = x_start + i * LAYER_GAP
            col.shift(np.array([x, 0, 0]))
            shifted_centres = [c + np.array([x, 0, 0]) for c in centres]
            columns.append(col)
            col_centres.append(shifted_centres)

        # Lines between adjacent columns.
        meshes: list[VGroup] = []
        for i in range(n_cols - 1):
            meshes.append(_connect(col_centres[i], col_centres[i + 1]))

        # --- Intro: input layer appears -----------------------------------
        self.play(FadeIn(columns[0]), run_time=intro)

        # --- Main: forward pass -------------------------------------------
        # Each step: lines from prev to current, then current circles.
        per_step = main / max(1, n_cols - 1)
        for i in range(1, n_cols):
            self.play(
                Create(meshes[i - 1]),
                run_time=max(0.4, per_step * 0.55),
            )
            self.play(
                FadeIn(columns[i]),
                run_time=max(0.3, per_step * 0.45),
            )

        # --- Outro: hold ---------------------------------------------------
        self.wait(outro)

    def _render_placeholder(self) -> None:
        intro, main, outro = phases(self.run_seconds)
        msg = Text(
            "Insufficient layers for net diagram",
            color=THEME_FOREGROUND,
        ).scale(0.5)
        self.play(FadeIn(msg), run_time=intro)
        self.wait(main + outro)
