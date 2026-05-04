"""Common scaffolding for every Phase G.4 template.

Each template:
  * is a `TemplateScene` subclass (Manim `Scene` with two extra
    instance attributes: `params` from the LLM and `run_seconds`
    from the planner).
  * exports a module-level `render(...)` helper or, more commonly,
    relies on the shared `render(scene_cls, params, duration_ms,
    output_path)` defined here.

The shared render helper handles:
  * Manim's `tempconfig` overrides (resolution / fps / colour /
    output file).
  * Per-render tempdir so concurrent renders don't collide on
    Manim's intermediate files.
  * Moving the produced MP4 to the requested final path (Manim's
    own output layout shifts between releases; we glob for any
    `*.mp4` under the tempdir to stay forward-compatible).

Theme colours mirror the Revideo path's ``library`` preset so a
chapter that mixes Manim segments with Revideo / fast-path segments
doesn't visually jolt at the seam.
"""

from __future__ import annotations

import shutil
import tempfile
from pathlib import Path
from typing import Tuple, Type

from manim import Scene, tempconfig

# ---------------------------------------------------------------------------
# Theme — keep in sync with backend/render/src/themes/index.ts (library
# preset) and backend/api/src/animation/fast_path.rs (LIBRARY palette).
# Hex strings are easier on Manim's `background_color` config knob;
# Manim accepts ``#RRGGBB`` directly.
# ---------------------------------------------------------------------------

THEME_BACKGROUND = "#0F172A"   # deep slate (matches library preset)
THEME_FOREGROUND = "#FFFFFF"   # white text on dark background
THEME_ACCENT = "#F59E0B"       # amber — used for highlights / callouts
THEME_DIM = "#475569"          # muted slate — secondary axes / grid

# ---------------------------------------------------------------------------
# Default render settings. The Rust publisher passes the duration in
# milliseconds; everything else is fixed at the project level so all
# Manim segments concat losslessly with Revideo / fast-path segments.
# ---------------------------------------------------------------------------

DEFAULT_FPS = 30
DEFAULT_WIDTH = 1920
DEFAULT_HEIGHT = 1080

# Floor on per-segment runtime. Anything shorter than this and Manim's
# fades / writes look rushed; we extend silently so the LLM doesn't
# have to think about pacing in the visual_params it emits.
MIN_RUN_SECONDS = 2.0


class TemplateScene(Scene):
    """Base for every G.4 template.

    Subclasses access:
      * ``self.params``       — the LLM's `visual_params` dict.
      * ``self.run_seconds``  — total render runtime, seconds.
      * ``self.theme()``      — palette + font dict; see method docstring.

    The driver attaches ``params`` and ``run_seconds`` as instance
    attributes before invoking ``scene.render()`` — Manim's
    ``Scene.__init__`` doesn't accept extra args, so subclasses
    must read them from ``self``.
    """

    params: dict
    run_seconds: float

    def theme(self) -> dict:
        """Return the active theme palette + font family.

        Surfaced as a method (not a module-level import) so
        LLM-authored Scene classes can read the colours without
        breaking the AST screen's import allowlist — the
        ``manim_code`` prompt advertises this exact API.

        Keys mirror the Revideo / fast-path ``library`` palette so
        Manim segments concat seamlessly with prose / image segments
        in the chapter timeline.
        """
        return {
            "bg": THEME_BACKGROUND,
            "primary": THEME_FOREGROUND,
            "accent": THEME_ACCENT,
            "text_primary": THEME_FOREGROUND,
            "text_secondary": THEME_DIM,
            "font": "Inter",
        }


def phases(run_seconds: float, weights: Tuple[float, float, float] = (0.2, 0.6, 0.2)) -> Tuple[float, float, float]:
    """Split ``run_seconds`` into intro / main / outro at the given
    weights. Floors each to 0.3 s so even very short renders still
    have a discernible reveal + hold + fade.

    Used by every template to pace its three-act structure
    consistently. The default weights (20 / 60 / 20) put the bulk of
    visible time on the actual diagram, which is what we want.
    """
    total = max(MIN_RUN_SECONDS, run_seconds)
    a, b, c = weights
    s = a + b + c
    if s <= 0:
        a, b, c = 0.2, 0.6, 0.2
        s = 1.0
    intro = max(0.3, total * (a / s))
    outro = max(0.3, total * (c / s))
    main = max(0.5, total - intro - outro)
    return intro, main, outro


def render(
    scene_cls: Type[TemplateScene],
    params: dict,
    duration_ms: int,
    output_path: Path,
) -> Path:
    """Render ``scene_cls(params)`` to ``output_path`` (MP4).

    Returns the resolved output path on success; raises on any Manim
    error (the G.5 sidecar wraps that into the NDJSON `error` event).
    The caller is responsible for ensuring `output_path.parent`
    exists if it cares — this helper creates it defensively.
    """

    output_path = Path(output_path).resolve()
    output_path.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="listenai-manim-") as tmp:
        overrides = {
            "media_dir": tmp,
            "output_file": output_path.stem,
            "format": "mp4",
            "pixel_width": DEFAULT_WIDTH,
            "pixel_height": DEFAULT_HEIGHT,
            "frame_rate": DEFAULT_FPS,
            "quality": "high_quality",
            "verbosity": "ERROR",
            "write_to_movie": True,
            "disable_caching": True,
            "background_color": THEME_BACKGROUND,
        }
        with tempconfig(overrides):
            scene = scene_cls()
            scene.params = params or {}
            scene.run_seconds = max(MIN_RUN_SECONDS, duration_ms / 1000.0)
            scene.render()

        # Manim writes to <media_dir>/videos/<scene>/<quality>/<file>.mp4
        # but the layout has shifted between releases. Glob for the
        # one MP4 we just produced and move it to the requested path.
        for produced in Path(tmp).rglob("*.mp4"):
            shutil.move(str(produced), str(output_path))
            return output_path

    raise RuntimeError(
        f"manim render produced no MP4 for {scene_cls.__name__} (params={params!r})"
    )
