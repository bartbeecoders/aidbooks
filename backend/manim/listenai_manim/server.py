"""Long-lived Manim render sidecar (Phase G.5 + H).

Mirrors the Node sidecar's NDJSON-over-stdio protocol but in Python
and specialised to the per-template render shape Phase G.6 needs.

Two request shapes share the same protocol, discriminated by ``kind``
(default ``"template"`` so existing callers don't need to change):

  * ``kind: "template"`` — pick a pre-defined Scene class out of
    ``listenai_manim.templates`` and render it with the given params.
  * ``kind: "raw_scene"`` — exec an LLM-generated ``Scene`` class
    after AST-screening it for forbidden imports/calls. See the
    ``raw_scene`` module for the screen rules.

Protocol — stdin: one render request JSON per line.

    # Template path (Phase G.5):
    {
        "version": 1,
        "kind": "template",
        "template_id": "function_plot",
        "params": {"fn": "x**2", "domain": [-3, 3]},
        "duration_ms": 12000,
        "output_mp4": "/abs/path/to/seg.mp4"
    }

    # Raw-scene path (Phase H):
    {
        "version": 1,
        "kind": "raw_scene",
        "code": "class Scene(TemplateScene):\\n    def construct(self):\\n        ...",
        "duration_ms": 12000,
        "output_mp4": "/abs/path/to/seg.mp4"
    }

Closing stdin = graceful shutdown. The sidecar processes any
in-flight render to completion and emits ``bye`` before exit.

Protocol — stdout: NDJSON events. One JSON object per line.

    {"type": "ready"}                                   # boot + between renders
    {"type": "started"}                                 # request accepted
    {"type": "done", "mp4": "...", "duration_ms": ...}  # render success
    {"type": "error", "message": "..."}                 # request failed
    {"type": "bye"}                                     # shutdown

Per-render failures stay non-fatal: emit ``error`` then ``ready``
and wait for the next request. Spec parse errors and unknown
templates also use this path.

# Why we hijack fd 1 before imports

Manim renders by shelling out to `xelatex`, `dvisvgm`, and `ffmpeg`,
plus its own tqdm progress bars. All of those write to file
descriptor 1, which is *the* stdout — not the Python `sys.stdout`
attribute. The Rust pool reads fd 1 as NDJSON; any rogue byte from
a subprocess corrupts the protocol.

Fix: capture a dup of fd 1 into a private fd, then `dup2` fd 2 over
fd 1 so anyone (Python code, subprocesses) writing to "stdout"
actually lands on stderr. The captured fd is what `emit()` writes
to, exclusively.

This has to happen *before* any import that might write to stdout
during module load (numpy, manim, manimpango), so the file is
structured: hijack fd → import heavy modules → start the loop.
"""

from __future__ import annotations

import json
import os
import sys

# ---------------------------------------------------------------------------
# fd 1 hijack. Must be the very first thing in this module — even
# `print()` calls in heavy imports below would otherwise leak.
# ---------------------------------------------------------------------------

_REAL_STDOUT_FD: int = os.dup(1)
os.dup2(2, 1)
# Replace Python's sys.stdout too. The fd hijack catches subprocess
# bytes; this catches Python-level `print()` and `sys.stdout.write`
# that other libraries do at module-load time.
sys.stdout = os.fdopen(2, "w", buffering=1, closefd=False)


def emit(event: dict) -> None:
    """Write one NDJSON event to the real (captured) stdout.

    Always followed by a newline. Flushes immediately so the Rust
    pool sees events in real time, not at process exit.
    """
    line = json.dumps(event, ensure_ascii=False) + "\n"
    os.write(_REAL_STDOUT_FD, line.encode("utf-8"))


# ---------------------------------------------------------------------------
# Now safe to import heavy modules — they may write to stdout/print
# during init, all of which now lands on stderr.
# ---------------------------------------------------------------------------

from pathlib import Path  # noqa: E402

from .raw_scene import RawSceneError, render_raw_scene  # noqa: E402
from .templates import TEMPLATES, render  # noqa: E402

EXPECTED_VERSION = 1


def _validate_common(req: dict) -> tuple[int, Path] | str:
    """Pull + sanity-check the fields shared by template and raw-scene
    requests: version, duration_ms, output_mp4. Returns the validated
    pair or an error string.
    """
    version = req.get("version")
    if version != EXPECTED_VERSION:
        return f"unsupported version: {version!r} (expected {EXPECTED_VERSION})"

    duration_raw = req.get("duration_ms", 0)
    try:
        duration_ms = int(duration_raw)
    except (TypeError, ValueError):
        return f"duration_ms must be a number, got {duration_raw!r}"
    if duration_ms <= 0:
        return f"duration_ms must be positive, got {duration_ms}"

    output_mp4 = req.get("output_mp4")
    if not isinstance(output_mp4, str) or not output_mp4.strip():
        return "output_mp4 must be a non-empty string"
    if not output_mp4.endswith(".mp4"):
        return f"output_mp4 must end with .mp4 (got {output_mp4!r})"

    return duration_ms, Path(output_mp4)


def _validate_template_request(req: dict) -> tuple[str, dict, int, Path] | str:
    """Validate a `kind: "template"` request. Returns the four-tuple
    or an error string."""
    common = _validate_common(req)
    if isinstance(common, str):
        return common
    duration_ms, output_path = common

    template_id = req.get("template_id")
    if not isinstance(template_id, str) or template_id not in TEMPLATES:
        return f"unknown template_id: {template_id!r}"

    params = req.get("params") or {}
    if not isinstance(params, dict):
        return f"params must be an object, got {type(params).__name__}"

    return template_id, params, duration_ms, output_path


def _validate_raw_scene_request(req: dict) -> tuple[str, int, Path] | str:
    """Validate a `kind: "raw_scene"` request. Returns the three-tuple
    ``(code, duration_ms, output_path)`` or an error string. The AST
    screen runs at exec time inside ``render_raw_scene``."""
    common = _validate_common(req)
    if isinstance(common, str):
        return common
    duration_ms, output_path = common

    code = req.get("code")
    if not isinstance(code, str):
        return f"code must be a string, got {type(code).__name__}"
    if not code.strip():
        return "code is empty"

    return code, duration_ms, output_path


def _handle_request(req: dict) -> None:
    """Drive one render. Always emits exactly one terminal event
    (``done`` or ``error``) per call. The caller emits ``ready``
    between calls.

    Dispatches on the `kind` discriminator (default ``"template"``
    so older Rust pools don't need to change to keep working).
    """
    kind = req.get("kind", "template")

    if kind == "template":
        parsed = _validate_template_request(req)
        if isinstance(parsed, str):
            emit({"type": "error", "message": parsed})
            return

        template_id, params, duration_ms, output_path = parsed

        emit({"type": "started"})

        try:
            scene_cls = TEMPLATES[template_id]
            out_path = render(scene_cls, params, duration_ms, output_path)
        except Exception as exc:  # noqa: BLE001 — every Manim failure mode
            emit({
                "type": "error",
                "message": f"{type(exc).__name__}: {exc}",
            })
            return

        emit({
            "type": "done",
            "mp4": str(out_path),
            "duration_ms": duration_ms,
        })
        return

    if kind == "raw_scene":
        parsed = _validate_raw_scene_request(req)
        if isinstance(parsed, str):
            emit({"type": "error", "message": parsed})
            return

        code, duration_ms, output_path = parsed

        emit({"type": "started"})

        try:
            out_path = render_raw_scene(code, duration_ms, output_path)
        except RawSceneError as exc:
            emit({"type": "error", "message": f"raw_scene: {exc}"})
            return
        except Exception as exc:  # noqa: BLE001
            emit({
                "type": "error",
                "message": f"raw_scene: {type(exc).__name__}: {exc}",
            })
            return

        emit({
            "type": "done",
            "mp4": str(out_path),
            "duration_ms": duration_ms,
        })
        return

    emit({"type": "error", "message": f"unknown kind: {kind!r}"})


def main() -> int:
    """Sidecar main loop.

    Reads NDJSON requests from stdin, processes them sequentially,
    emits NDJSON events on stdout. Closes stdin → ``bye`` and exit.
    """
    emit({"type": "ready"})

    for raw_line in sys.stdin:
        line = raw_line.strip()
        if not line:
            continue

        try:
            req = json.loads(line)
        except json.JSONDecodeError as exc:
            emit({"type": "error", "message": f"invalid JSON: {exc}"})
            emit({"type": "ready"})
            continue

        if not isinstance(req, dict):
            emit({"type": "error", "message": "request must be a JSON object"})
            emit({"type": "ready"})
            continue

        try:
            _handle_request(req)
        except Exception as exc:  # noqa: BLE001 — keep loop alive
            emit({
                "type": "error",
                "message": f"sidecar threw: {type(exc).__name__}: {exc}",
            })
        emit({"type": "ready"})

    emit({"type": "bye"})
    return 0


if __name__ == "__main__":
    sys.exit(main())
