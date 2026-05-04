"""Smoke scene used to verify the Manim install end-to-end.

Renders a ~4-second 1080p MP4 that exercises:

  * Plain text via Pango  (catches a missing `libpango1.0-dev`)
  * `MathTex` math via LaTeX + `dvisvgm`  (catches missing `texlive-*`)
  * `Axes` + `plot()` to build confidence the templates in G.4 will
    have what they need.

Successful run = the toolchain is good enough for G.4. Failure means
something on the host is missing; the README under
`docs/animation/render-host.md` lists what to install per distro.

Invoke with:

    cd backend/manim
    uv run listenai-manim-smoke

…or via the justfile recipe (`just manim-smoke`). Output lands at
`output/<scene>.mp4` next to this file (Manim's default media dir).
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

from manim import (
    BLUE,
    DOWN,
    UP,
    Axes,
    Create,
    FadeIn,
    FadeOut,
    MathTex,
    Scene,
    Text,
    Write,
)


class SmokeScene(Scene):
    """Three-act smoke: title, equation reveal, plot.

    Kept deliberately simple — every Manim primitive used here is one
    that the G.4 templates will rely on, so a clean run here is a
    strong signal that templates won't trip on the toolchain.
    """

    def construct(self) -> None:
        # Act 1 — Pango text. Fails when libpango isn't installed.
        title = Text("ListenAI Manim smoke", font_size=48)
        self.play(Write(title), run_time=0.8)
        self.wait(0.4)
        self.play(FadeOut(title), run_time=0.4)

        # Act 2 — LaTeX math. Fails when texlive packages are missing
        # or when dvisvgm can't be found on PATH.
        equation = MathTex(r"e^{i\pi} + 1 = 0", font_size=80)
        self.play(Write(equation), run_time=0.8)
        self.wait(0.6)
        self.play(FadeOut(equation), run_time=0.3)

        # Act 3 — Axes + plot. Confirms numpy <-> manim integration is
        # working, which is what every G.4 function_plot template
        # depends on.
        axes = Axes(x_range=(-3, 3, 1), y_range=(-1.5, 1.5, 1), tips=False)
        graph = axes.plot(lambda x: 0.5 * x**2 - 1, color=BLUE)
        label = MathTex(r"f(x) = \tfrac{1}{2}x^{2} - 1").next_to(axes, UP)
        self.play(Create(axes), run_time=0.4)
        self.play(Create(graph), FadeIn(label, shift=DOWN * 0.2), run_time=0.6)
        self.wait(0.6)


def main() -> int:
    """Entry point for `listenai-manim-smoke`.

    Renders `SmokeScene` headlessly at 1080p30 into the smoke
    directory under `backend/manim/`. Exits 0 on success, 1 on any
    Manim error. Caller (justfile / smoke runner) verifies the MP4
    actually landed.
    """

    # Manim's `tempconfig` lets us override CLI flags inline. Same
    # settings as `manim -qh`, plus a fixed output directory so the
    # smoke verifier can find the file deterministically.
    from manim import config, tempconfig

    here = Path(__file__).resolve().parent.parent  # backend/manim/
    out_dir = here / "smoke_output"
    out_dir.mkdir(parents=True, exist_ok=True)

    overrides = {
        "media_dir": str(out_dir),
        "output_file": "smoke",
        "format": "mp4",
        "pixel_width": 1920,
        "pixel_height": 1080,
        "frame_rate": 30,
        "quality": "high_quality",
        "verbosity": "ERROR",
        "write_to_movie": True,
        "disable_caching": True,
    }
    try:
        with tempconfig(overrides):
            scene = SmokeScene()
            scene.render()
    except Exception as exc:  # noqa: BLE001 — surface anything to stderr
        print(f"smoke render failed: {exc}", file=sys.stderr)
        return 1

    # Manim writes to <media_dir>/videos/<module>/<quality>/<file>.mp4.
    # Walk the tree to find it rather than hard-coding Manim's layout
    # (which has shifted in past releases).
    for root, _dirs, files in os.walk(out_dir):
        for name in files:
            if name.endswith(".mp4"):
                print(Path(root) / name)
                return 0

    print("smoke render completed but no MP4 was produced", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
