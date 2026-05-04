"""``bar_chart`` template — categorical comparison.

Param shape:

    { "data": [{"label": "...", "value": <number>}, ...] }   # 2 ≤ N ≤ 8

Bars rise sequentially in input order so the eye lands on the
running comparison rather than a static finished chart. Manim's
built-in ``BarChart`` handles the layout; we add the staggered
reveal manually because BarChart's default animation doesn't.
"""

from __future__ import annotations

from typing import Iterable

from manim import (
    BarChart,
    Create,
    DOWN,
    FadeIn,
    Text,
    UP,
)

from ._base import THEME_ACCENT, THEME_DIM, THEME_FOREGROUND, TemplateScene, phases


class BarChartScene(TemplateScene):
    """See module docstring for params."""

    def construct(self) -> None:
        raw_data = self.params.get("data", [])
        if not isinstance(raw_data, Iterable):
            raw_data = []

        labels: list[str] = []
        values: list[float] = []
        for entry in raw_data:
            if not isinstance(entry, dict):
                continue
            label = entry.get("label")
            value = entry.get("value")
            if not isinstance(label, str) or not label.strip():
                continue
            try:
                v = float(value)
            except (TypeError, ValueError):
                continue
            labels.append(label.strip())
            values.append(v)
            if len(labels) >= 8:
                break

        if len(labels) < 2:
            # Degenerate — render a centred title saying so. Better
            # than crashing or producing a one-bar chart.
            self._render_placeholder()
            return

        intro, main, outro = phases(self.run_seconds)

        # Manim's BarChart auto-fits the y-axis. We initialise with
        # zero values then `.change_bar_values` to animate the rise.
        bars = BarChart(
            values=[0.0] * len(values),
            bar_names=labels,
            y_range=[0, max(values) * 1.15, max(1.0, max(values) / 4.0)],
            x_length=10,
            y_length=4.5,
            bar_colors=[THEME_ACCENT],
            bar_fill_opacity=0.85,
            x_axis_config={"color": THEME_DIM, "include_numbers": False},
            y_axis_config={
                "color": THEME_DIM,
                "include_numbers": True,
                "include_ticks": True,
            },
        )

        # Intro: empty axes appear ----------------------------------------
        self.play(Create(bars), run_time=intro)

        # Main: bars rise sequentially -----------------------------------
        per_bar = max(0.3, main / len(values))
        # change_bar_values takes the full target list, so we ratchet
        # one at a time by passing partials.
        for i in range(len(values)):
            target = [0.0] * len(values)
            for j in range(i + 1):
                target[j] = values[j]
            bars.change_bar_values(target)
            self.play(
                Create(bars),  # redraws to the new heights
                run_time=per_bar * 0.85,
            )

        # Outro: hold ------------------------------------------------------
        self.wait(outro)

    def _render_placeholder(self) -> None:
        """Fallback when there's not enough data to chart."""
        intro, main, outro = phases(self.run_seconds)
        msg = Text(
            "Insufficient data for bar chart",
            color=THEME_FOREGROUND,
        ).scale(0.5)
        self.play(FadeIn(msg), run_time=intro)
        self.wait(main + outro)
