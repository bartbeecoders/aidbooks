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
