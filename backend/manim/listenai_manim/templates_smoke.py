"""Render one MP4 per G.4 template into ``smoke_output/templates/``.

Drives every template with a representative ``visual_params`` shape
and a fixed 4-second run. Used for eyeball QA — `just
manim-templates-smoke` invokes this. Successful run = a directory of
8 MP4s each named after its visual_kind.

Each entry below is the *minimum* sample input the template should
gracefully render. Do *not* depend on this file from the Rust side
(the LLM provides real params at runtime); it's QA scaffolding only.
"""

from __future__ import annotations

import sys
from pathlib import Path

from .templates import TEMPLATES, render

# 4 seconds is short enough to render the whole suite in a couple of
# minutes total but long enough that each template's reveal /
# main / outro phases all show up.
DEFAULT_DURATION_MS = 4_000

# Representative params per visual_kind. Each shape mirrors what
# `paragraph_visual_v1.md` documents — keep them in sync.
SAMPLES: dict[str, dict] = {
    "function_plot": {
        "fn": "x**2 - 2*x + 1",
        "domain": [-3, 3],
        "emphasize": "vertex",
    },
    "axes_with_curve": {
        "x_label": "time",
        "y_label": "concentration",
        "curve_kind": "exp",
        "emphasize": "growth",
    },
    "vector_field": {"description": "rotational flow around the origin"},
    "free_body": {
        "object": "block on incline",
        "forces": ["gravity", "normal", "friction"],
    },
    "flow_chart": {
        "steps": ["observe", "hypothesise", "experiment", "analyse", "conclude"],
    },
    "bar_chart": {
        "data": [
            {"label": "H₂", "value": 1.0},
            {"label": "CH₄", "value": 16.0},
            {"label": "C₂H₆", "value": 30.0},
            {"label": "CO₂", "value": 44.0},
        ],
    },
    "equation_steps": {
        "steps": [
            r"F = m \cdot a",
            r"a = F / m",
        ],
    },
    "neural_net_layer": {"neurons": [3, 4, 4, 2]},
}


def main() -> int:
    here = Path(__file__).resolve().parent.parent  # backend/manim/
    out_dir = here / "smoke_output" / "templates"
    out_dir.mkdir(parents=True, exist_ok=True)

    failures: list[str] = []
    for kind, scene_cls in TEMPLATES.items():
        params = SAMPLES.get(kind, {})
        out_path = out_dir / f"{kind}.mp4"
        print(f"[render] {kind:<18} → {out_path}")
        try:
            render(scene_cls, params, DEFAULT_DURATION_MS, out_path)
        except Exception as exc:  # noqa: BLE001 — surface every failure
            print(f"  ✗ {kind}: {exc}", file=sys.stderr)
            failures.append(kind)
            continue
        if not out_path.exists():
            print(f"  ✗ {kind}: render returned but no MP4 at {out_path}", file=sys.stderr)
            failures.append(kind)

    if failures:
        print(
            f"\n{len(failures)} of {len(TEMPLATES)} templates failed: "
            f"{', '.join(failures)}",
            file=sys.stderr,
        )
        return 1

    print(f"\nAll {len(TEMPLATES)} templates rendered to {out_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
