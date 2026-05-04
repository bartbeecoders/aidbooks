set shell := ["bash", "-cu"]
set dotenv-load := true

default:
    @just --list

# --- Install / setup -------------------------------------------------------

setup:
    cd frontend && npm install

# --- Dev -------------------------------------------------------------------

# Run backend (hot reload requires cargo-watch: `cargo install cargo-watch`)
dev-backend:
    cd backend && cargo run -p api

dev-backend-watch:
    cd backend && cargo watch -x 'run -p api'

dev-frontend:
    cd frontend && npm run dev

# Run both concurrently (requires GNU parallel or two terminals in practice;
# easiest is two shells, but this works if you have `concurrently` via npx).
dev:
    npx --yes concurrently -k -n backend,frontend -c blue,magenta "just dev-backend" "just dev-frontend"

# --- Quality ---------------------------------------------------------------

fmt:
    cd backend && cargo fmt --all
    cd frontend && npm run format

lint:
    cd backend && cargo fmt --all -- --check
    cd backend && cargo clippy --all-targets --all-features -- -D warnings
    cd frontend && npm run lint

typecheck:
    cd frontend && npm run typecheck

test:
    cd backend && cargo test --all
    cd frontend && npm test --if-present

check: lint typecheck test

# --- Animation render harness ---------------------------------------------

# Smoke-test the Revideo renderer end-to-end. Generates a synthetic 12s
# WAV + waveform + SceneSpec, drives backend/render/dist/cli.js, and
# verifies the MP4 with ffprobe. Requires `just animate-build` first.
animate-test:
    cd backend/render && npm run test:fixture

# Same but skips Chromium/Revideo and produces a black-frame MP4 via
# ffmpeg. Useful for proving the wiring works on a host that can't run
# the full renderer (CI, headless dev VMs).
animate-test-mock:
    cd backend/render && npm run test:fixture:mock

# Build the Node sidecar (one-time setup before `animate-test` or before
# starting the backend with LISTENAI_ANIMATE_RENDERER_CMD set).
animate-build:
    cd backend/render && npm install && npm run build

# End-to-end demo against a running backend: logs in as demo admin,
# creates a tiny audiobook, narrates it, animates it, opens the result.
# Backend must be running with LISTENAI_DEV_SEED=true and either
# LISTENAI_ANIMATE_RENDERER_CMD set or LISTENAI_ANIMATE_MOCK=true.
animate-demo *ARGS:
    scripts/animate-demo.sh {{ARGS}}

# --- Manim diagram path (Phase G) -----------------------------------------

# Create the Python venv + sync deps. Requires `uv` (recommended:
# `curl -LsSf https://astral.sh/uv/install.sh | sh`) and the system
# packages from docs/animation/render-host.md (LaTeX + Cairo + Pango).
# The env-strip + PATH override route around an active conda env's
# compiler / lib leakage; harmless when conda isn't active.
manim-build:
    cd backend/manim && env \
        -u CC -u CXX -u CPP -u FC \
        -u LD -u AR -u AS -u NM -u RANLIB -u STRIP \
        -u CFLAGS -u CPPFLAGS -u CXXFLAGS -u LDFLAGS \
        -u DEBUG_CFLAGS -u DEBUG_CPPFLAGS -u DEBUG_CXXFLAGS \
        -u HOST -u BUILD -u CONDA_BUILD_SYSROOT \
        -u CMAKE_PREFIX_PATH -u CMAKE_ARGS \
        -u GCC -u GXX -u GCC_AR -u GCC_NM -u GCC_RANLIB \
        -u LD_LIBRARY_PATH -u PKG_CONFIG_PATH \
        PATH="/usr/bin:/bin:$PATH" \
        uv sync --python-preference managed

# Run the toolchain smoke. Renders a 4-second MP4 that exercises Pango
# text + LaTeX MathTex + Axes/plot. Output lands under
# backend/manim/smoke_output/. ~30s first run, 5-15s subsequent.
# LD_PRELOAD forces system libfontconfig ahead of the older one the
# manimpango wheel bundles — without it, Pango fails to import on Arch.
manim-smoke:
    cd backend/manim && env \
        -u CC -u CXX -u CPP \
        -u CFLAGS -u CPPFLAGS -u CXXFLAGS -u LDFLAGS \
        -u LD_LIBRARY_PATH -u PKG_CONFIG_PATH \
        PATH="/usr/bin:/bin:$PATH" \
        LD_PRELOAD="/usr/lib/libfontconfig.so.1" \
        uv run listenai-manim-smoke

# Render one MP4 per G.4 template into backend/manim/smoke_output/
# templates/. Same env-strip + LD_PRELOAD as the toolchain smoke.
# Takes a couple of minutes for the full eight-template suite; useful
# for eyeball QA before promoting to the live render path.
manim-templates-smoke:
    cd backend/manim && env \
        -u CC -u CXX -u CPP \
        -u CFLAGS -u CPPFLAGS -u CXXFLAGS -u LDFLAGS \
        -u LD_LIBRARY_PATH -u PKG_CONFIG_PATH \
        PATH="/usr/bin:/bin:$PATH" \
        LD_PRELOAD="/usr/lib/libfontconfig.so.1" \
        uv run listenai-manim-templates-smoke

# Smoke the G.5 NDJSON sidecar end-to-end: spawns the server,
# pipes 2 render requests at it, verifies events + MP4s land, then
# closes stdin and checks for a clean `bye`. ~30s total.
manim-server-smoke:
    cd backend/manim && env \
        -u CC -u CXX -u CPP \
        -u CFLAGS -u CPPFLAGS -u CXXFLAGS -u LDFLAGS \
        -u LD_LIBRARY_PATH -u PKG_CONFIG_PATH \
        PATH="/usr/bin:/bin:$PATH" \
        LD_PRELOAD="/usr/lib/libfontconfig.so.1" \
        uv run listenai-manim-server-smoke

# Build the OCI image (~3 GB; 3-5 min cold, ~30s incremental). Tag is
# `listenai-manim:0.1` so subsequent G.5/G.6 work can refer to it by
# a stable name.
manim-container-build:
    podman build -f backend/manim/Containerfile -t listenai-manim:0.1 backend/manim

# Run the smoke inside the container. Uses :Z relabelling for
# SELinux-enabled distros; drop it on systems without SELinux if it
# causes issues. Output lands in backend/manim/smoke_output/ on the
# host so the verifier can find it.
manim-container-smoke:
    mkdir -p backend/manim/smoke_output
    podman run --rm \
        -v "$PWD/backend/manim/smoke_output:/home/manim/smoke_output:Z" \
        listenai-manim:0.1 \
        python -m listenai_manim.smoke

# --- Build -----------------------------------------------------------------

build:
    cd backend && cargo build --release
    cd frontend && npm run build

clean:
    cd backend && cargo clean
    rm -rf frontend/dist frontend/node_modules/.vite
