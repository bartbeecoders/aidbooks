# `backend/manim/` — Phase G Manim diagram renderer

The Python half of the animation pipeline. Companion to
`backend/render/` (Node + Revideo), but specialised for **STEM
diagrams**: function plots, free-body diagrams, vector fields,
equation transforms, etc.

## Quick start

```sh
# Native dev (Arch / EndeavourOS) — one-shot install + smoke:
scripts/manim-install.sh

# Or step-by-step, after the system deps are in place:
just manim-build      # uv sync into .venv
just manim-smoke      # renders backend/manim/smoke_output/.../smoke.mp4

# Container (podman, prod / CI)
just manim-container-build         # builds listenai-manim:0.1
just manim-container-smoke         # runs the smoke inside the image
```

A successful smoke produces a 1080p30 MP4 with a Pango title fade,
an `e^{i\\pi} + 1 = 0` equation reveal, and an `Axes + plot()` curve
of `f(x) = ½x² − 1`. If anything fails, every primitive is
intentional — see the docstring in `listenai_manim/smoke.py` for
which dependency a given failure points at.

## Layout

```
backend/manim/
├── Containerfile            — podman/docker image (LaTeX + Manim 0.18)
├── pyproject.toml           — uv / hatchling project metadata
├── requirements.txt         — plain-pip equivalent (used by the container)
├── README.md                — this file
└── listenai_manim/
    ├── __init__.py
    ├── smoke.py             — smoke scene used to validate the toolchain
    └── (G.4) templates/     — function_plot, free_body, etc. — empty stubs
    └── (G.5) server.py      — NDJSON sidecar driven by the Rust publisher
```

## Why the separate package + container?

Same reasons as `backend/render/`:

- **Toolchain isolation** — Manim's deps (LaTeX especially) are heavy
  and version-sensitive. A container guarantees prod renders look the
  same as dev renders.
- **Crash isolation** — bad LaTeX in a `MathTex` doesn't bring down
  the Rust API; the sidecar process eats the failure and emits a
  structured error event.
- **Swap-ability** — the contract is `stdin: NDJSON spec, stdout:
  NDJSON progress, file: mp4`. The Rust side doesn't care whether the
  renderer is Manim CE today or 3Blue1Brown's manimgl tomorrow.

## Phase G roadmap

| Sub-phase | Status | What lands |
|---|---|---|
| G.3 | this | Toolchain + skeleton + smoke |
| G.4 | next | Per-`visual_kind` templates under `listenai_manim/templates/` |
| G.5 | after | `listenai_manim/server.py` — NDJSON sidecar, mirror of `backend/render/src/server.ts` |
| G.6 | after | Per-segment chapter assembler (Manim segments concat with fast-path segments) |

See `docs/animation/render-host.md` for the full operations doc.
