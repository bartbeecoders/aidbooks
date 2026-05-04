#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID_DIR="$ROOT/.pids"
LOG_DIR="$ROOT/.logs"
BACKEND_PID_FILE="$PID_DIR/backend.pid"
FRONTEND_PID_FILE="$PID_DIR/frontend.pid"
MCP_PID_FILE="$PID_DIR/mcp.pid"
BACKEND_LOG_FILE="$LOG_DIR/backend.log"
FRONTEND_LOG_FILE="$LOG_DIR/frontend.log"
MCP_LOG_FILE="$LOG_DIR/mcp.log"

mkdir -p "$PID_DIR" "$LOG_DIR"

# Load .env into this shell so LISTENAI_* vars reach `cargo run` below.
# `set -a` auto-exports every assignment until `set +a`. Without this the
# backend falls back to mock mode for LLM + TTS even when keys are present.
if [[ -f "$ROOT/.env" ]]; then
    echo "Loading $ROOT/.env"
    set -a
    # shellcheck disable=SC1091
    source "$ROOT/.env"
    set +a
else
    echo "No .env found at $ROOT/.env (backend will use defaults / mock mode)."
fi

# MCP HTTP transport — defaults to listening on every interface so other
# machines on the LAN can reach it. Override with LISTENAI_MCP_BIND in .env
# (e.g. `LISTENAI_MCP_BIND=127.0.0.1:8788`) to restrict to loopback.
# Anyone who can reach this port can drive the audiobook backend, so leave
# LISTENAI_TOKEN unset on the server side and require clients to authenticate
# (Authorization: Bearer <jwt> or `_token` arg).
: "${LISTENAI_MCP_BIND:=0.0.0.0:8788}"
export LISTENAI_MCP_BIND

# Phase G — Manim diagram render path (STEM only). Re-sync the venv
# on every boot so newly-added console scripts (e.g. `listenai-manim-
# server`) land on disk without the user having to remember a
# separate command. Cheap when nothing changed (`uv sync` is a no-op
# in that case); fails loudly if `uv` isn't installed but doesn't
# block backend startup.
sync_manim_venv() {
    local manim_dir="$ROOT/backend/manim"
    if [[ ! -f "$manim_dir/pyproject.toml" ]]; then
        return 0
    fi

    # uv installs to ~/.local/bin by default; some shells don't have
    # it on PATH unless explicitly added. Prepend defensively.
    export PATH="$HOME/.local/bin:$PATH"
    if ! command -v uv >/dev/null 2>&1; then
        echo "Note: uv not on PATH — skipping Manim venv sync."
        echo "  Install once with: curl -LsSf https://astral.sh/uv/install.sh | sh"
        echo "  Then re-run dev.sh; the STEM diagram path is otherwise inert."
        return 0
    fi

    echo "Syncing Manim venv (backend/manim/.venv)..."
    # Strip conda's compiler/lib env vars so a `(base)`-active shell
    # doesn't leak `x86_64-conda-linux-gnu-cc` + conda's -L paths
    # into the source build of `manimpango`. Same wrapper the
    # `just manim-build` recipe uses.
    if (
        cd "$manim_dir" && \
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
            uv sync --python-preference managed
    ); then
        # Belt-and-braces: `uv sync` has been observed to register
        # console scripts in `.venv/bin/` without actually
        # installing the local package into site-packages, leaving
        # the scripts to crash with `ModuleNotFoundError` at
        # runtime. Always run the editable install — it's
        # idempotent + cheap when the package is already present,
        # and a guaranteed fix when it isn't. (We can't reliably
        # *probe* whether listenai_manim is installed by running
        # `python -c "import listenai_manim"` because Python adds
        # the CWD to sys.path implicitly, so the import succeeds
        # via the local source dir even when the package isn't in
        # site-packages — false-negative for the conditional.)
        if [[ -x "$manim_dir/.venv/bin/python" ]]; then
            # uv-created venvs don't ship `pip`, so we go through
            # `uv pip install`. `--python <venv-python>` is more
            # direct than `VIRTUAL_ENV` discovery: tells uv exactly
            # where site-packages lives. `--no-deps` keeps it
            # cheap; idempotent.
            #
            # No `--quiet` — if the install fails we want to see
            # *why* in the dev.sh terminal, not have it swallowed.
            if (
                cd "$manim_dir" && \
                env \
                    -u CC -u CXX -u CPP -u FC \
                    -u CFLAGS -u CPPFLAGS -u CXXFLAGS -u LDFLAGS \
                    -u LD_LIBRARY_PATH -u PKG_CONFIG_PATH \
                    PATH="$HOME/.local/bin:/usr/bin:/bin:$PATH" \
                    uv pip install --python .venv/bin/python -e . --no-deps
            ); then
                : # success
            else
                echo "  ⚠ uv pip install -e . failed; STEM diagram path will not work."
            fi
        fi
        echo "Manim venv ready."
    else
        echo "Manim venv sync failed — continuing without it."
        echo "  STEM segment-mode renders will warn-and-fall-back to prose."
    fi
}

sync_manim_venv

stop_server() {
    local name="$1"
    local pid_file="$2"

    if [[ -f "$pid_file" ]]; then
        local pid
        pid=$(cat "$pid_file")
        if kill -0 "$pid" 2>/dev/null; then
            echo "Stopping $name (PID $pid)..."
            kill "$pid"
            local waited=0
            while kill -0 "$pid" 2>/dev/null && (( waited < 10 )); do
                sleep 1
                waited=$(( waited + 1 ))
            done
            if kill -0 "$pid" 2>/dev/null; then
                echo "Force-killing $name (PID $pid)..."
                kill -9 "$pid"
            fi
            echo "$name stopped."
        else
            echo "$name PID file found but process is not running."
        fi
        rm -f "$pid_file"
    fi
}

stop_all() {
    stop_server "backend" "$BACKEND_PID_FILE"
    stop_server "frontend" "$FRONTEND_PID_FILE"
    stop_server "mcp" "$MCP_PID_FILE"
    # Belt-and-braces: anything with the same binary name that the PID file
    # didn't catch (orphan from a previous run whose shell died, or a manual
    # `cargo run` started outside this script) would still hold the RocksDB
    # LOCK file and crash the next backend boot. Sweep by name as a final
    # cleanup. `pgrep -f` matches the full command, `-x` is too strict here.
    for bin in "target/debug/listenai-api" "target/debug/listenai-mcp"; do
        local stale
        stale=$(pgrep -f "$bin" || true)
        if [[ -n "$stale" ]]; then
            echo "Found orphan $bin processes: $stale — killing"
            # shellcheck disable=SC2086
            kill $stale 2>/dev/null || true
            sleep 1
            stale=$(pgrep -f "$bin" || true)
            if [[ -n "$stale" ]]; then
                echo "Force-killing $stale"
                # shellcheck disable=SC2086
                kill -9 $stale 2>/dev/null || true
            fi
        fi
    done
}

cleanup() {
    echo ""
    echo "Shutting down servers..."
    stop_all
    exit 0
}

trap cleanup SIGINT SIGTERM

stop_all

# Truncate previous log files so each `dev.sh` run starts clean. The
# `>&` and `&` combo tees stdout+stderr to the file *and* the terminal so
# you can watch live or `tail -f .logs/backend.log` later.
: > "$BACKEND_LOG_FILE"
: > "$FRONTEND_LOG_FILE"
: > "$MCP_LOG_FILE"

echo "Starting backend..."
cd "$ROOT/backend"
# `2>&1 | tee` keeps both streams + survives terminal close (the cargo
# process still writes to the file via the pipe).
{ cargo run -p api 2>&1 | tee -a "$BACKEND_LOG_FILE"; } &
BACKEND_PID=$!
echo "$BACKEND_PID" > "$BACKEND_PID_FILE"
echo "Backend started (PID $BACKEND_PID, logs: $BACKEND_LOG_FILE)."

# The MCP server fetches /openapi.json at boot and exits fatally if the api
# is not yet listening. Wait for the api to be reachable before launching it.
# Cap the wait so a broken backend doesn't hang dev.sh forever.
API_BASE="${LISTENAI_API_URL:-http://127.0.0.1:8787}"
echo "Waiting for backend at $API_BASE/health ..."
WAITED=0
until curl -fsS --max-time 1 "$API_BASE/health" >/dev/null 2>&1; do
    if ! kill -0 "$BACKEND_PID" 2>/dev/null; then
        echo "Backend exited before becoming ready — see $BACKEND_LOG_FILE"
        stop_all
        exit 1
    fi
    if (( WAITED >= 180 )); then
        echo "Backend did not become ready within 180s — see $BACKEND_LOG_FILE"
        stop_all
        exit 1
    fi
    sleep 1
    WAITED=$(( WAITED + 1 ))
done
echo "Backend is ready (took ${WAITED}s)."

echo "Starting MCP server (http on $LISTENAI_MCP_BIND)..."
cd "$ROOT/backend"
{ cargo run -p mcp -- --http 2>&1 | tee -a "$MCP_LOG_FILE"; } &
MCP_PID=$!
echo "$MCP_PID" > "$MCP_PID_FILE"
echo "MCP server started (PID $MCP_PID, logs: $MCP_LOG_FILE)."

echo "Starting frontend..."
cd "$ROOT/frontend"
{ npm run dev 2>&1 | tee -a "$FRONTEND_LOG_FILE"; } &
FRONTEND_PID=$!
echo "$FRONTEND_PID" > "$FRONTEND_PID_FILE"
echo "Frontend started (PID $FRONTEND_PID, logs: $FRONTEND_LOG_FILE)."

echo ""
echo "All servers are running. Press Ctrl+C to stop."
echo "Tail logs in another terminal with:"
echo "  tail -f $BACKEND_LOG_FILE"
echo "  tail -f $MCP_LOG_FILE"
echo "  tail -f $FRONTEND_LOG_FILE"

wait $BACKEND_PID $MCP_PID $FRONTEND_PID
