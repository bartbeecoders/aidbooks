"""LLM-generated Manim Scene execution path (Phase H).

The structured-template path (`templates/`) handles 99% of diagrams the
classifier picks. When a paragraph is genuinely outside any template's
shape, the classifier marks it `visual_kind="custom_manim"` and a
code-gen LLM writes a bespoke ``Scene`` class for that paragraph. This
module is what the sidecar runs on the resulting code:

  1. **Parse** with ``ast.parse``. Reject anything that doesn't even
     parse (syntax errors etc.).

  2. **Screen** the AST against an allowlist of imports + a denylist of
     dangerous calls (``__import__``, ``eval``, ``exec``, ``open``).
     The screen walks every node in the tree, not just module-level —
     a hidden ``import os`` inside a function body is still rejected.

  3. **Exec** the code into a fresh namespace pre-populated with
     ``TemplateScene`` + ``from manim import *`` + ``math`` + ``np``.
     Look for a class named ``Scene`` that subclasses
     ``TemplateScene``; if missing, raise.

  4. **Render** the scene with the standard ``render(...)`` helper
     from ``templates._base``, which handles the tempconfig + output
     plumbing identically to the template path.

The screen is the security floor. It is not a sandbox — a sufficiently
motivated attacker can defeat it (e.g. ``getattr(builtins, "ev"+"al")``
chains). For an open-internet deployment, the manim sidecar should run
inside the project's podman ``Containerfile`` instead, with no network
+ a tmpfs scratch dir. On a single-developer workstation, the screen
is enough to catch the LLM emitting something dumb (``import os`` while
trying to read its own source) without false positives.
"""

from __future__ import annotations

import ast
from pathlib import Path
from typing import Any

from .templates._base import TemplateScene, render

# ---------------------------------------------------------------------------
# Allowlists / denylists. The screen is conservative on purpose: the
# code-gen prompt instructs the LLM to use only these imports, so the
# screen rarely needs to reject. Any rejection is an early signal that
# the prompt drifted or the LLM is going off-script — surface it as a
# loud error rather than silently letting the code through.
# ---------------------------------------------------------------------------

ALLOWED_IMPORT_ROOTS: frozenset[str] = frozenset({
    "manim",
    "math",
    "numpy",
})

# Modules we explicitly refuse, even though they wouldn't pass the
# allowlist anyway. Listed for the better error message they produce
# ("forbidden import: os") versus the generic allowlist message.
FORBIDDEN_IMPORT_ROOTS: frozenset[str] = frozenset({
    "os",
    "sys",
    "subprocess",
    "socket",
    "pathlib",
    "requests",
    "urllib",
    "http",
    "shutil",
    "tempfile",
    "ctypes",
    "importlib",
    "builtins",
    "asyncio",
})

FORBIDDEN_CALL_NAMES: frozenset[str] = frozenset({
    "__import__",
    "eval",
    "exec",
    "compile",
    "open",
    "getattr",  # closes the getattr-by-name escape hatch
    "globals",
    "locals",
    "vars",
})


class RawSceneError(Exception):
    """Raised on any failure in the raw-scene pipeline. The sidecar
    catches this and emits an ``error`` event verbatim."""


def _root_module(name: str) -> str:
    """Return the top-level package name. ``"numpy.linalg"`` → ``"numpy"``."""
    return name.split(".", 1)[0] if name else ""


def screen_code(code: str) -> None:
    """Raise ``RawSceneError`` if the source is unsafe to ``exec``.

    Walks the AST and rejects on:
      * any ``import x`` / ``from x import y`` where ``x`` is in the
        forbidden list, or where the root isn't on the allowlist;
      * any call to a denylisted builtin (``open``, ``eval``, …);
      * any attribute access into the forbidden roots
        (``os.system(...)`` is dead even if ``os`` was somehow imported
        by a prior pass).
    """

    try:
        tree = ast.parse(code)
    except SyntaxError as exc:
        raise RawSceneError(f"syntax error: {exc.msg} at line {exc.lineno}") from exc

    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                root = _root_module(alias.name)
                if root in FORBIDDEN_IMPORT_ROOTS:
                    raise RawSceneError(f"forbidden import: {alias.name}")
                if root not in ALLOWED_IMPORT_ROOTS:
                    raise RawSceneError(
                        f"import not on allowlist: {alias.name} "
                        f"(allowed: {sorted(ALLOWED_IMPORT_ROOTS)})"
                    )
        elif isinstance(node, ast.ImportFrom):
            root = _root_module(node.module or "")
            if not root:
                # `from . import x` — disallow relative imports outright,
                # the LLM has no business reaching into sibling modules.
                raise RawSceneError("relative imports are not allowed")
            if root in FORBIDDEN_IMPORT_ROOTS:
                raise RawSceneError(f"forbidden import: {node.module}")
            if root not in ALLOWED_IMPORT_ROOTS:
                raise RawSceneError(
                    f"import not on allowlist: {node.module} "
                    f"(allowed: {sorted(ALLOWED_IMPORT_ROOTS)})"
                )
        elif isinstance(node, ast.Call):
            # Catch ``open(...)`` / ``eval(...)`` / etc. Attribute calls
            # like ``os.system(...)`` are caught by the Attribute branch.
            if isinstance(node.func, ast.Name) and node.func.id in FORBIDDEN_CALL_NAMES:
                raise RawSceneError(f"forbidden call: {node.func.id}(...)")
        elif isinstance(node, ast.Attribute):
            # Walk a potentially-chained attribute back to its root
            # ``Name`` and check the root against the forbidden list.
            cur = node.value
            while isinstance(cur, ast.Attribute):
                cur = cur.value
            if isinstance(cur, ast.Name) and cur.id in FORBIDDEN_IMPORT_ROOTS:
                raise RawSceneError(
                    f"forbidden attribute access: {cur.id}.{node.attr}"
                )


def render_raw_scene(code: str, duration_ms: int, output_mp4: Path) -> Path:
    """End-to-end: screen → exec → locate ``Scene`` → render.

    On any failure raises ``RawSceneError``. On success returns the
    resolved output MP4 path.
    """

    if not isinstance(code, str) or not code.strip():
        raise RawSceneError("code is empty")

    screen_code(code)

    # Pre-populate the namespace so the LLM doesn't need to bother with
    # imports the screen would have tolerated anyway. Mirrors what the
    # template files do at the top of their modules.
    namespace: dict[str, Any] = {}
    # `from manim import *` — populate namespace explicitly to avoid
    # ``exec`` having to re-run the import each call (slow + noisy).
    import math

    import numpy as np
    import manim

    namespace.update({k: getattr(manim, k) for k in dir(manim) if not k.startswith("_")})
    namespace["TemplateScene"] = TemplateScene
    namespace["math"] = math
    namespace["np"] = np

    try:
        exec(  # noqa: S102 — screened above
            compile(code, "<llm-manim>", "exec"),
            namespace,
            namespace,
        )
    except Exception as exc:  # noqa: BLE001
        raise RawSceneError(f"exec failed: {type(exc).__name__}: {exc}") from exc

    scene_cls = namespace.get("Scene")
    if scene_cls is None:
        raise RawSceneError(
            "no class named `Scene` was defined in the LLM code"
        )
    if not (isinstance(scene_cls, type) and issubclass(scene_cls, TemplateScene)):
        raise RawSceneError(
            "`Scene` must be a class extending TemplateScene"
        )

    return render(scene_cls, params={}, duration_ms=duration_ms, output_path=output_mp4)
