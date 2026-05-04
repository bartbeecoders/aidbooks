#!/usr/bin/env bash
#
# Phase G — Manim diagram-render toolchain install (EndeavourOS / Arch).
#
# What this does, in order:
#   1. Sanity-checks the host: Arch-based distro, pacman + sudo
#      present, repo layout matches what the script expects.
#   2. `sudo pacman -S --needed` the system deps Manim CE pulls in:
#      python + ffmpeg + cairo/pango + the texlive packages we need
#      for Pango text rendering and LaTeX MathTex (kept by user
#      decision — see plan-animation.md G.3).
#   3. Installs `uv` (Astral's fast Python package manager) if it
#      isn't already on PATH. Falls back to a pip-based path with a
#      printed warning if `uv` install fails.
#   4. `uv sync` inside `backend/manim/` to materialise `.venv`
#      against the project's `pyproject.toml`.
#   5. Runs `listenai-manim-smoke` to prove the toolchain end-to-end
#      (Pango text + LaTeX MathTex + Axes/plot). Verifies an MP4
#      lands and prints its path + size + duration.
#
# Usage:
#   scripts/manim-install.sh              # full install + smoke
#   scripts/manim-install.sh --no-smoke   # just install, skip render
#   scripts/manim-install.sh --no-system  # skip pacman; assume deps present
#   scripts/manim-install.sh --help
#
# Re-run safe: every step is idempotent. Re-running after fixing a
# missing package on PATH will pick up where the failure was.

set -euo pipefail

# --- Argument parsing -----------------------------------------------------

SKIP_SMOKE=false
SKIP_SYSTEM=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-smoke)   SKIP_SMOKE=true; shift ;;
        --no-system)  SKIP_SYSTEM=true; shift ;;
        -h|--help)
            sed -n '3,28p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *)
            echo "Unknown arg: $1" >&2
            echo "Try --help" >&2
            exit 2 ;;
    esac
done

# --- Helpers --------------------------------------------------------------

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIM_DIR="$ROOT/backend/manim"

step() { echo; echo "==> $*"; }
ok()   { echo "    ✓ $*"; }
warn() { echo "    ⚠ $*" >&2; }
fail() { echo "    ✗ $*" >&2; exit 1; }

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        fail "required command '$1' not found on PATH"
    fi
}

# --- 1. Pre-flight --------------------------------------------------------

step "Pre-flight checks"

if [[ ! -f "$MANIM_DIR/pyproject.toml" ]]; then
    fail "Expected $MANIM_DIR/pyproject.toml — am I being run from the wrong repo?"
fi
ok "repo layout looks right"

if [[ ! -f /etc/os-release ]]; then
    fail "no /etc/os-release; can't detect distro"
fi
# shellcheck disable=SC1091
. /etc/os-release
case "${ID:-}-${ID_LIKE:-}" in
    *arch*|*endeavouros*)
        ok "distro is Arch-based: ${PRETTY_NAME:-$ID}"
        ;;
    *)
        if $SKIP_SYSTEM; then
            warn "distro is not Arch-based (${PRETTY_NAME:-$ID}); --no-system was given so we'll continue"
        else
            fail "distro is not Arch-based (${PRETTY_NAME:-$ID}); pass --no-system if you'll install deps yourself"
        fi
        ;;
esac

require bash
require curl

if ! $SKIP_SYSTEM; then
    require pacman
    require sudo
    ok "pacman + sudo on PATH"
fi

# --- 2. System packages ---------------------------------------------------

# Pacman package set:
#   python                       → interpreter (Manim CE wants 3.11+)
#   ffmpeg                       → encode pipeline
#   cairo, pango                 → glyph + 2D rasterisation libs
#   texlive-basic                → minimal LaTeX kernel
#   texlive-latexrecommended     → amsmath, amssymb, etc.
#   texlive-latexextra           → standalone.cls, pgfplots, …
#                                  (Manim's default preamble uses
#                                  \documentclass[preview]{standalone})
#   texlive-pictures             → TikZ / PGF (transitive dep of
#                                  some Manim text paths)
#   texlive-fontsrecommended     → Computer Modern fonts
#   texlive-mathscience          → physics, siunitx, mathtools
#   texlive-binextra             → dvisvgm (Manim's DVI→SVG step)
PACMAN_PKGS=(
    python
    ffmpeg
    cairo
    pango
    texlive-basic
    texlive-latexrecommended
    texlive-latexextra
    texlive-pictures
    texlive-fontsrecommended
    texlive-mathscience
    texlive-binextra
)

if $SKIP_SYSTEM; then
    step "Skipping system packages (--no-system)"
else
    step "Installing system packages via pacman (sudo will prompt)"
    echo "    Packages: ${PACMAN_PKGS[*]}"
    sudo pacman -S --needed --noconfirm "${PACMAN_PKGS[@]}" \
        || fail "pacman install failed — resolve conflicts and re-run"
    ok "system packages installed"
fi

# --- 3. uv (Astral's Python package manager) ------------------------------

step "Locating uv"

if ! command -v uv >/dev/null 2>&1; then
    warn "uv not found on PATH — running Astral's installer"
    if curl -LsSf https://astral.sh/uv/install.sh | sh; then
        # Astral's installer drops the binary in $HOME/.local/bin
        # (or $XDG_DATA_HOME/uv on some setups). Add the conventional
        # path to this shell so the subsequent `uv sync` can find it,
        # then warn the user to update their rc files persistently.
        export PATH="$HOME/.local/bin:$PATH"
        if command -v uv >/dev/null 2>&1; then
            ok "uv installed at $(command -v uv)"
            warn "Add '$HOME/.local/bin' to your shell PATH so 'just manim-*' recipes find uv"
        else
            fail "uv installer reported success but uv still not on PATH"
        fi
    else
        fail "uv installer failed; fall back to plain pip per docs/animation/render-host.md"
    fi
else
    ok "uv already on PATH at $(command -v uv)"
fi

# --- 4. Sync the venv -----------------------------------------------------

# manimpango (pulled in by Manim) only ships prebuilt wheels for
# CPython 3.9–3.12. On Python 3.13+ uv falls back to a source build,
# which fails inside an active conda env because the conda toolchain's
# linker doesn't see the system Pango/Cairo libraries (`-lpango-1.0`
# not found, etc.). Pinning to a uv-managed 3.11 here sidesteps both
# problems regardless of what's on PATH or CONDA_DEFAULT_ENV.

if [[ -n "${CONDA_DEFAULT_ENV:-}" ]]; then
    warn "conda env '$CONDA_DEFAULT_ENV' is active; uv will use its own managed Python (ignoring conda)"
fi

step "Ensuring uv-managed Python 3.11 is available"
uv python install 3.11 \
    || fail "uv python install 3.11 failed — check network access"
ok "Python 3.11 installed (or already present)"

step "Creating venv + syncing deps via uv"
# `--python-preference managed` tells uv to prefer its own download
# over any system / conda interpreter, and the project's
# `.python-version` file pins to 3.11 so the choice is deterministic.
#
# We also strip conda's compiler env vars before invoking `uv` so
# any C extension uv decides to build from source falls back to
# system gcc + /usr/lib (where libpango/libcairo actually live)
# rather than conda's gcc + /home/bart/miniconda3/lib (where they
# don't). Belt-and-braces with `only-binary = ["manimpango",
# "pycairo"]` in pyproject.toml: even if uv ignores that knob, the
# clean build env will still produce a working library.
( cd "$MANIM_DIR" && \
    env \
        -u CC -u CXX -u CPP -u FC \
        -u LD -u AR -u AS -u NM -u RANLIB -u STRIP \
        -u CFLAGS -u CPPFLAGS -u CXXFLAGS -u LDFLAGS \
        -u DEBUG_CFLAGS -u DEBUG_CPPFLAGS -u DEBUG_CXXFLAGS \
        -u HOST -u BUILD -u CONDA_BUILD_SYSROOT \
        -u CMAKE_PREFIX_PATH -u CMAKE_ARGS \
        -u GCC -u GXX -u GCC_AR -u GCC_NM -u GCC_RANLIB \
        -u LD_LIBRARY_PATH -u PKG_CONFIG_PATH \
        PATH="/usr/bin:/bin:$PATH" \
        uv sync --python-preference managed ) \
    || fail "uv sync failed — see output above (often a missing system lib or the conda env override)"
ok "venv ready at $MANIM_DIR/.venv"

# --- 5. Smoke -------------------------------------------------------------

if $SKIP_SMOKE; then
    step "Skipping smoke (--no-smoke)"
    echo "    Run 'just manim-smoke' or 'cd backend/manim && uv run listenai-manim-smoke' when ready."
    exit 0
fi

step "Running toolchain smoke (1080p30, ~30s first time)"

# Capture full stdout + stderr to a log so we have something to
# show the user on failure. The smoke prints its output mp4 path
# on its last stdout line; we read that back from the log.
SMOKE_LOG="$(mktemp -t manim-smoke.XXXXXX.log)"
trap 'rm -f "$SMOKE_LOG"' EXIT

set +e
# `LD_PRELOAD` forces the system libfontconfig (which has
# `FcConfigSetDefaultSubstitute`) ahead of whatever older one the
# manimpango wheel bundled. Without this, system libpangoft2 fails
# to resolve at import time on Arch.
( cd "$MANIM_DIR" && \
    env \
        -u CC -u CXX -u CPP \
        -u CFLAGS -u CPPFLAGS -u CXXFLAGS -u LDFLAGS \
        -u LD_LIBRARY_PATH -u PKG_CONFIG_PATH \
        PATH="/usr/bin:/bin:$PATH" \
        LD_PRELOAD="/usr/lib/libfontconfig.so.1${LD_PRELOAD:+:$LD_PRELOAD}" \
        uv run listenai-manim-smoke ) \
    >"$SMOKE_LOG" 2>&1
SMOKE_RC=$?
set -e

if [[ $SMOKE_RC -ne 0 ]]; then
    echo "    --- smoke output (tail) ---" >&2
    tail -n 40 "$SMOKE_LOG" | sed 's/^/    /' >&2
    echo "    --- end smoke output ---" >&2
    fail "smoke exited $SMOKE_RC; full log: $SMOKE_LOG"
fi

# Smoke prints the path of the rendered mp4 on its last stdout line.
SMOKE_OUTPUT_PATH="$(tail -n 1 "$SMOKE_LOG")"
if [[ ! -f "$SMOKE_OUTPUT_PATH" ]]; then
    # Fall back to walking the output dir if the last-line trick
    # picked up something unexpected (extra trailing log line, etc.).
    SMOKE_OUTPUT_PATH="$(find "$MANIM_DIR/smoke_output" -name '*.mp4' 2>/dev/null | head -1)"
fi

if [[ -n "$SMOKE_OUTPUT_PATH" && -f "$SMOKE_OUTPUT_PATH" ]]; then
    ok "smoke MP4 at $SMOKE_OUTPUT_PATH"
else
    echo "    --- smoke output (tail) ---" >&2
    tail -n 40 "$SMOKE_LOG" | sed 's/^/    /' >&2
    echo "    --- end smoke output ---" >&2
    fail "smoke ran but no MP4 produced; full log: $SMOKE_LOG"
fi

# --- 6. Verify (best effort, optional ffprobe) ----------------------------

if command -v ffprobe >/dev/null 2>&1; then
    DURATION="$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$SMOKE_OUTPUT_PATH" 2>/dev/null || echo unknown)"
    SIZE_HUMAN="$(numfmt --to=iec --suffix=B "$(stat -c '%s' "$SMOKE_OUTPUT_PATH")" 2>/dev/null || echo unknown)"
    ok "duration: ${DURATION}s, size: $SIZE_HUMAN"
fi

echo
echo "Done. Toolchain ready:"
echo "  Native:    just manim-smoke"
echo "  Container: just manim-container-build && just manim-container-smoke"
echo "  Docs:      docs/animation/render-host.md"
