#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID_DIR="$ROOT/.pids"
LOG_DIR="$ROOT/.logs"
BACKEND_PID_FILE="$PID_DIR/backend.pid"
FRONTEND_PID_FILE="$PID_DIR/frontend.pid"
BACKEND_LOG_FILE="$LOG_DIR/backend.log"
FRONTEND_LOG_FILE="$LOG_DIR/frontend.log"

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
    # Belt-and-braces: anything with the same binary name that the PID file
    # didn't catch (orphan from a previous run whose shell died, or a manual
    # `cargo run` started outside this script) would still hold the RocksDB
    # LOCK file and crash the next backend boot. Sweep by name as a final
    # cleanup. `pgrep -f` matches the full command, `-x` is too strict here.
    local stale
    stale=$(pgrep -f "target/debug/listenai-api" || true)
    if [[ -n "$stale" ]]; then
        echo "Found orphan backend processes: $stale — killing"
        # shellcheck disable=SC2086
        kill $stale 2>/dev/null || true
        sleep 1
        stale=$(pgrep -f "target/debug/listenai-api" || true)
        if [[ -n "$stale" ]]; then
            echo "Force-killing $stale"
            # shellcheck disable=SC2086
            kill -9 $stale 2>/dev/null || true
        fi
    fi
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

echo "Starting backend..."
cd "$ROOT/backend"
# `2>&1 | tee` keeps both streams + survives terminal close (the cargo
# process still writes to the file via the pipe).
{ cargo run -p api 2>&1 | tee -a "$BACKEND_LOG_FILE"; } &
BACKEND_PID=$!
echo "$BACKEND_PID" > "$BACKEND_PID_FILE"
echo "Backend started (PID $BACKEND_PID, logs: $BACKEND_LOG_FILE)."

echo "Starting frontend..."
cd "$ROOT/frontend"
{ npm run dev 2>&1 | tee -a "$FRONTEND_LOG_FILE"; } &
FRONTEND_PID=$!
echo "$FRONTEND_PID" > "$FRONTEND_PID_FILE"
echo "Frontend started (PID $FRONTEND_PID, logs: $FRONTEND_LOG_FILE)."

echo ""
echo "Both servers are running. Press Ctrl+C to stop."
echo "Tail logs in another terminal with:"
echo "  tail -f $BACKEND_LOG_FILE"
echo "  tail -f $FRONTEND_LOG_FILE"

wait $BACKEND_PID $FRONTEND_PID
