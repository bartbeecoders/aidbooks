#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID_DIR="$ROOT/.pids"
LOG_DIR="$ROOT/.logs"
BACKEND_PID_FILE="$PID_DIR/backend.pid"
FRONTEND_PID_FILE="$PID_DIR/frontend.pid"
MCP_PID_FILE="$PID_DIR/mcp.pid"
MOLD_PID_FILE="$PID_DIR/mold.pid"
MOLD_SERVICE_PID_FILE="$PID_DIR/mold-service.pid"
BACKEND_LOG_FILE="$LOG_DIR/backend.log"
FRONTEND_LOG_FILE="$LOG_DIR/frontend.log"
MCP_LOG_FILE="$LOG_DIR/mcp.log"
MOLD_LOG_FILE="$LOG_DIR/mold.log"
MOLD_SERVICE_LOG_FILE="$LOG_DIR/mold-service.log"

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

# Self-hosted mold image-gen server. Bound to loopback by default — the
# admin LLM rows talk to it via `base_url`, so there's no reason to
# expose it to the LAN unless you're sharing the GPU. Override with
# LISTENAI_MOLD_BIND / LISTENAI_MOLD_PORT in .env to widen.
: "${LISTENAI_MOLD_BIND:=127.0.0.1}"
: "${LISTENAI_MOLD_PORT:=7680}"
export LISTENAI_MOLD_BIND LISTENAI_MOLD_PORT

# mold-service — AidBooks' HTTP wrapper around `mold serve`. Owns the
# GPU semaphore, OOM cooldown, default model/steps/guidance, and the
# 9:16 shorts policy. The backend's `llm` rows with `provider = "mold"`
# should set `base_url = http://<bind>:<port>` here (not at mold serve
# directly). Override the bind/port with LISTENAI_MOLD_SERVICE_BIND /
# LISTENAI_MOLD_SERVICE_PORT in .env.
: "${LISTENAI_MOLD_SERVICE_BIND:=127.0.0.1}"
: "${LISTENAI_MOLD_SERVICE_PORT:=7681}"
export LISTENAI_MOLD_SERVICE_BIND LISTENAI_MOLD_SERVICE_PORT

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

# Lifted from `mold/scripts/examples.sh`. If `mold` is linked against a
# different CUDA major than the system has installed (binary built against
# CUDA 12 on a CUDA 13 host, etc.), look for a compatible libcublas
# alongside common pip-installed nvidia packages and prepend their lib
# dirs to LD_LIBRARY_PATH. Skipped entirely when `mold version` already
# runs cleanly — which it does on this host today, so this is defensive.
ensure_mold_cuda_libs() {
    if mold version >/dev/null 2>&1; then
        return 0
    fi
    local err
    err=$(mold version 2>&1 || true)
    [[ "$err" == *"libcublas.so."* ]] || return 0

    local needed
    needed=$(echo "$err" | grep -oE 'libcublas\.so\.[0-9]+' | head -1)
    echo "↪ mold needs $needed — searching for a compatible copy..." >&2

    local search_roots=(
        "$HOME/.local"
        "$HOME/miniconda3" "$HOME/anaconda3" "$HOME/.conda"
        "/opt" "/usr/local"
        "/run/media"
    )
    local found
    found=$(find "${search_roots[@]}" -maxdepth 8 -type f -name "$needed" \
        -path '*/nvidia/cublas/lib/*' 2>/dev/null | head -1)
    if [ -z "$found" ]; then
        found=$(find "${search_roots[@]}" -maxdepth 8 -type f -name "$needed" 2>/dev/null | head -1)
    fi
    if [ -z "$found" ]; then
        echo "  ✗ no $needed found — mold serve will not start." >&2
        echo "      Install a matching CUDA, or rebuild mold against the system CUDA." >&2
        return 1
    fi

    local cublas_dir nvidia_root extra=()
    cublas_dir=$(dirname "$found")
    if [[ "$cublas_dir" == */nvidia/cublas/lib ]]; then
        nvidia_root=$(dirname "$(dirname "$cublas_dir")")
        for d in "$nvidia_root"/*/lib; do
            [ -d "$d" ] && extra+=("$d")
        done
    fi
    local prepend
    prepend=$(IFS=:; echo "${extra[*]:-$cublas_dir}")
    export LD_LIBRARY_PATH="$prepend${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    echo "  ✓ using $cublas_dir" >&2
}

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
    stop_server "mold-service" "$MOLD_SERVICE_PID_FILE"
    stop_server "mold" "$MOLD_PID_FILE"
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
: > "$MOLD_LOG_FILE"
: > "$MOLD_SERVICE_LOG_FILE"

echo "Starting backend..."
cd "$ROOT/backend"
# `2>&1 | tee` keeps both streams + survives terminal close (the cargo
# process still writes to the file via the pipe).
{ cargo run -p api 2>&1 | tee -a "$BACKEND_LOG_FILE"; } &
BACKEND_PID=$!
echo "$BACKEND_PID" > "$BACKEND_PID_FILE"
echo "Backend started (PID $BACKEND_PID, logs: $BACKEND_LOG_FILE)."

# Self-hosted image-gen server. Independent of the backend — the API
# only contacts it on-demand when an `llm` row with `provider = "mold"`
# is selected, so a failed-to-start mold doesn't break the rest of
# dev.sh. We warm it up in parallel with the backend so model load
# (lazy, on first request) doesn't block the first generation.
MOLD_PID=""
if command -v mold >/dev/null 2>&1; then
    ensure_mold_cuda_libs
    echo "Starting mold serve on $LISTENAI_MOLD_BIND:$LISTENAI_MOLD_PORT..."
    cd "$ROOT"
    {
        mold serve \
            --bind "$LISTENAI_MOLD_BIND" \
            --port "$LISTENAI_MOLD_PORT" \
            --log-format text \
            2>&1 | tee -a "$MOLD_LOG_FILE"
    } &
    MOLD_PID=$!
    echo "$MOLD_PID" > "$MOLD_PID_FILE"
    echo "mold serve started (PID $MOLD_PID, logs: $MOLD_LOG_FILE)."
else
    echo "Note: mold not on PATH — skipping mold serve."
    echo "  Image-gen via mold-provider LLM rows will fail until it's installed."
fi

# mold-service: HTTP wrapper that the backend talks to instead of
# hitting mold serve directly. Owns the GPU semaphore + OOM cooldown +
# default model/steps/guidance/dimensions. Skips when the crate isn't
# present (e.g. on a clean clone before `mold-service` is added) so the
# rest of dev.sh still works.
MOLD_SERVICE_PID=""
if [[ -f "$ROOT/mold-service/Cargo.toml" ]]; then
    echo "Starting mold-service on $LISTENAI_MOLD_SERVICE_BIND:$LISTENAI_MOLD_SERVICE_PORT..."
    cd "$ROOT/mold-service"
    {
        MOLD_SERVICE_BIND="$LISTENAI_MOLD_SERVICE_BIND" \
        MOLD_SERVICE_PORT="$LISTENAI_MOLD_SERVICE_PORT" \
        MOLD_UPSTREAM_URL="http://$LISTENAI_MOLD_BIND:$LISTENAI_MOLD_PORT" \
            cargo run --release 2>&1 | tee -a "$MOLD_SERVICE_LOG_FILE"
    } &
    MOLD_SERVICE_PID=$!
    echo "$MOLD_SERVICE_PID" > "$MOLD_SERVICE_PID_FILE"
    echo "mold-service started (PID $MOLD_SERVICE_PID, logs: $MOLD_SERVICE_LOG_FILE)."
    cd "$ROOT"
else
    echo "Note: mold-service/ not present — skipping."
fi

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
if [[ -n "$MOLD_PID" ]]; then
    echo "  tail -f $MOLD_LOG_FILE"
fi
if [[ -n "$MOLD_SERVICE_PID" ]]; then
    echo "  tail -f $MOLD_SERVICE_LOG_FILE"
fi

# `wait` accepts a list of PIDs but cannot take an empty arg — only
# include each optional PID when it actually started.
wait $BACKEND_PID $MCP_PID $FRONTEND_PID \
    ${MOLD_PID:+$MOLD_PID} \
    ${MOLD_SERVICE_PID:+$MOLD_SERVICE_PID}
