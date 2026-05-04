"""``free_body`` template — free-body diagram.

Param shape:

    {
      "object": "<noun phrase, e.g. 'block on incline'>",
      "forces": ["gravity", "normal", "friction", ...]
    }

Forces are drawn as arrows pointing in conventional physics
directions:

  * gravity     ↓
  * normal      ↑   (rotated for inclines? not in v1 — we keep it
                     vertical and let the narration explain)
  * friction    ←   (along surface, opposing motion)
  * applied     →   (default applied force direction)
  * tension     ↗   (along rope, default 30° up-right)
  * spring      ↑   (restoring; default upward)
  * thrust      →   (default forward)
  * drag        ←   (default backward)
  * lift        ↑   (aerodynamic lift)
  * weight      ↓   (synonym for gravity in some books)
  * (anything else) — cardinal slot, evenly spaced

The point of v1 is to show *which forces* act, not their magnitudes
or angles. Real per-scenario rotation will land in G.4.5 if needed.
"""

from __future__ import annotations

from typing import Iterable

import numpy as np
from manim import (
    Arrow,
    Create,
    DOWN,
    FadeIn,
    FadeOut,
    LEFT,
    ORIGIN,
    RIGHT,
    Square,
    Text,
    UP,
    VGroup,
    Write,
)

from ._base import THEME_ACCENT, THEME_DIM, THEME_FOREGROUND, TemplateScene, phases


# Conventional direction (unit vector) per common force name. Keys
# are lowercased; the template normalises before lookup. Anything
# missing here gets a slot-machine fallback that spaces unknown
# forces evenly around the object.
_DIRECTIONS: dict[str, tuple[float, float]] = {
    "gravity": (0, -1),
    "weight": (0, -1),
    "normal": (0, 1),
    "lift": (0, 1),
    "spring": (0, 1),
    "buoyancy": (0, 1),
    "friction": (-1, 0),
    "drag": (-1, 0),
    "applied": (1, 0),
    "thrust": (1, 0),
    "push": (1, 0),
    "tension": (np.cos(np.pi / 3), np.sin(np.pi / 3)),
    "torque": (np.cos(2 * np.pi / 3), np.sin(2 * np.pi / 3)),
}

# Fallback angles if the LLM emits a force we don't recognise. We
# walk this list as a slot machine: first unknown gets the first
# slot, second unknown gets the second, etc. Cardinal directions
# preferred so the diagram stays legible.
_FALLBACK_SLOTS = [
    (1, 0), (-1, 0), (0, 1), (0, -1),
    (np.cos(np.pi / 4), np.sin(np.pi / 4)),
    (np.cos(3 * np.pi / 4), np.sin(3 * np.pi / 4)),
    (np.cos(-np.pi / 4), np.sin(-np.pi / 4)),
    (np.cos(-3 * np.pi / 4), np.sin(-3 * np.pi / 4)),
]


def _direction_for(name: str, fallback_idx: int) -> tuple[float, float]:
    norm = name.strip().lower()
    if norm in _DIRECTIONS:
        return _DIRECTIONS[norm]
    return _FALLBACK_SLOTS[fallback_idx % len(_FALLBACK_SLOTS)]


class FreeBodyScene(TemplateScene):
    """See module docstring for params."""

    def construct(self) -> None:
        obj_name = str(self.params.get("object", "object")).strip() or "object"
        raw_forces = self.params.get("forces", [])
        if not isinstance(raw_forces, Iterable):
            raw_forces = []
        forces: list[str] = []
        for f in raw_forces:
            if isinstance(f, str) and f.strip():
                forces.append(f.strip())
            if len(forces) >= 6:  # cap diagram density
                break

        intro, main, outro = phases(self.run_seconds)

        # Central object — square with a label inside. Real free-body
        # diagrams treat the object as a point; we use a small square
        # so the label fits and the arrow tails have a clean origin.
        body = Square(side_length=1.0, color=THEME_FOREGROUND)
        body.set_fill(THEME_DIM, opacity=0.4)
        body_label = Text(obj_name, color=THEME_FOREGROUND).scale(0.32)
        if body_label.width > body.width * 0.85:
            body_label.scale(body.width * 0.85 / body_label.width)
        body_label.move_to(body.get_center())

        # Build each force arrow + label. Arrows start from the
        # square's edge in the force's direction (so they don't
        # overlap the body). Labels sit at the arrow's tip end.
        ARROW_LEN = 1.6
        BODY_RADIUS = 0.5  # half-side of the unit square
        unknown_idx = 0
        force_mobs: list[VGroup] = []
        for name in forces:
            dx, dy = _direction_for(name, unknown_idx)
            if name.strip().lower() not in _DIRECTIONS:
                unknown_idx += 1
            mag = (dx * dx + dy * dy) ** 0.5
            if mag == 0:
                continue
            ux, uy = dx / mag, dy / mag
            tail = np.array([ux * BODY_RADIUS, uy * BODY_RADIUS, 0.0])
            head = tail + ARROW_LEN * np.array([ux, uy, 0.0])
            arrow = Arrow(
                start=tail,
                end=head,
                buff=0,
                color=THEME_ACCENT,
                stroke_width=4,
                max_tip_length_to_length_ratio=0.18,
            )
            label = Text(name, color=THEME_FOREGROUND).scale(0.36)
            # Push the label slightly past the arrow tip so it doesn't
            # overlap the head.
            label.move_to(head + 0.35 * np.array([ux, uy, 0.0]))
            force_mobs.append(VGroup(arrow, label))

        # --- Intro: object appears -----------------------------------------
        self.play(Create(body), run_time=intro * 0.6)
        self.play(Write(body_label), run_time=intro * 0.4)

        # --- Main: arrows reveal one at a time -----------------------------
        if force_mobs:
            per_force = main / len(force_mobs)
            for fm in force_mobs:
                self.play(FadeIn(fm), run_time=max(0.3, per_force * 0.9))
                # Tiny gap between reveals so the eye can register
                # each force individually.
                if per_force > 0.5:
                    self.wait(per_force * 0.1)
        else:
            # No forces emitted — hold the body for the main slot so
            # total runtime still matches the planner.
            self.wait(main)

        self.wait(max(0.0, outro - 0.3))
        # Fade out the labelled arrows; leaving the body visible at
        # the seam reads better than a hard cut.
        if force_mobs:
            self.play(FadeOut(VGroup(*force_mobs)), run_time=0.3)
