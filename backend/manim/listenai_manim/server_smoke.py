"""End-to-end smoke for the NDJSON sidecar.

Spawns ``python -m listenai_manim.server`` as a subprocess, pipes a
small batch of render requests at it, and verifies:

  * The sidecar emits ``ready`` at boot.
  * Each request produces exactly one terminal event (``done`` or
    ``error``) followed by a fresh ``ready``.
  * Closing stdin elicits a ``bye`` and a clean exit code.
  * Each ``done`` event's ``mp4`` path actually exists on disk and is
    non-empty.

Driven by ``just manim-server-smoke`` and run after toolchain
installs to catch protocol regressions cheaply (no Rust required).
Total runtime ≈ 30 s for two short renders.

Per-request runtime is intentionally short here (3 s each) — the
goal is protocol coverage, not visual polish. The G.4 templates
smoke is the one to look at for visual QA.
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
from pathlib import Path


def _request_lines(out_dir: Path) -> list[str]:
    """Return one NDJSON request per template variant we want to
    exercise. Two requests is enough to cover the protocol's main
    code path (boot → ready → render → ready → render → ready →
    eof → bye).
    """
    requests = [
        {
            "version": 1,
            "template_id": "function_plot",
            "params": {"fn": "x**2", "domain": [-2, 2], "emphasize": "vertex"},
            "duration_ms": 3_000,
            "output_mp4": str(out_dir / "function_plot.mp4"),
        },
        {
            "version": 1,
            "template_id": "free_body",
            "params": {"object": "block", "forces": ["gravity", "normal"]},
            "duration_ms": 3_000,
            "output_mp4": str(out_dir / "free_body.mp4"),
        },
    ]
    return [json.dumps(r) + "\n" for r in requests]


def main() -> int:
    here = Path(__file__).resolve().parent.parent  # backend/manim/
    out_dir = here / "smoke_output" / "server"
    out_dir.mkdir(parents=True, exist_ok=True)
    # Wipe stale outputs so a previous half-failed run can't fool us.
    for old in out_dir.glob("*.mp4"):
        old.unlink()

    request_lines = _request_lines(out_dir)
    expected_done = len(request_lines)

    # Important: invoke the same Python the package is installed in
    # (the venv's interpreter, found via sys.executable) so the
    # spawned sidecar has the same listenai_manim + manim available.
    proc = subprocess.Popen(
        [sys.executable, "-m", "listenai_manim.server"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,  # line-buffered
    )
    assert proc.stdin is not None and proc.stdout is not None

    started_at = time.monotonic()
    events: list[dict] = []
    done_count = 0
    error_count = 0

    def _read_event() -> dict | None:
        line = proc.stdout.readline()
        if not line:
            return None
        try:
            evt = json.loads(line.strip())
        except json.JSONDecodeError as e:
            print(f"  ✗ non-NDJSON line on stdout: {line!r} ({e})", file=sys.stderr)
            return {"type": "_invalid", "raw": line}
        events.append(evt)
        return evt

    try:
        # 1. Wait for the boot `ready`.
        first = _read_event()
        if not first or first.get("type") != "ready":
            print(f"  ✗ expected boot ready, got {first!r}", file=sys.stderr)
            proc.kill()
            return 1
        print("  ✓ boot ready")

        # 2. For each request: send, read until next ready.
        for i, line in enumerate(request_lines, start=1):
            print(f"  → request {i}: {line.strip()[:80]}…")
            proc.stdin.write(line)
            proc.stdin.flush()

            saw_terminal = False
            while True:
                evt = _read_event()
                if evt is None:
                    print("  ✗ sidecar closed stdout mid-render", file=sys.stderr)
                    proc.kill()
                    return 1
                kind = evt.get("type")
                if kind == "started":
                    print("    [started]")
                elif kind == "done":
                    print(f"    [done] {evt.get('mp4')}")
                    done_count += 1
                    saw_terminal = True
                elif kind == "error":
                    print(f"    [error] {evt.get('message')}", file=sys.stderr)
                    error_count += 1
                    saw_terminal = True
                elif kind == "ready":
                    if not saw_terminal:
                        print(
                            "  ✗ ready arrived before terminal event",
                            file=sys.stderr,
                        )
                        proc.kill()
                        return 1
                    break
                else:
                    print(f"    [other] {evt!r}")

        # 3. Close stdin → expect bye + clean exit.
        proc.stdin.close()
        last = _read_event()
        if not last or last.get("type") != "bye":
            print(f"  ✗ expected bye on shutdown, got {last!r}", file=sys.stderr)
            proc.kill()
            return 1
        print("  ✓ shutdown bye")

        rc = proc.wait(timeout=10)
        if rc != 0:
            print(f"  ✗ sidecar exited {rc}", file=sys.stderr)
            return 1
    finally:
        if proc.poll() is None:
            proc.kill()

    elapsed = time.monotonic() - started_at

    # 4. Check the MP4s actually landed.
    missing: list[Path] = []
    for line in request_lines:
        req = json.loads(line)
        out = Path(req["output_mp4"])
        if not out.exists() or out.stat().st_size < 1024:
            missing.append(out)

    print(
        f"\nsummary: {done_count} done, {error_count} error, "
        f"{len(missing)} missing/empty mp4 — {elapsed:.1f}s elapsed"
    )

    if error_count > 0 or missing:
        return 1
    if done_count != expected_done:
        print(
            f"  ✗ expected {expected_done} done events, got {done_count}",
            file=sys.stderr,
        )
        return 1

    print(f"\nAll {expected_done} server-protocol renders OK ({out_dir})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
