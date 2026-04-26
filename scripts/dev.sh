#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID_DIR="$ROOT/.pids"
BACKEND_PID_FILE="$PID_DIR/backend.pid"
FRONTEND_PID_FILE="$PID_DIR/frontend.pid"

mkdir -p "$PID_DIR"

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
}

cleanup() {
    echo ""
    echo "Shutting down servers..."
    stop_all
    exit 0
}

trap cleanup SIGINT SIGTERM

stop_all

echo "Starting backend..."
cd "$ROOT/backend"
cargo run -p api &
BACKEND_PID=$!
echo "$BACKEND_PID" > "$BACKEND_PID_FILE"
echo "Backend started (PID $BACKEND_PID)."

echo "Starting frontend..."
cd "$ROOT/frontend"
npm run dev &
FRONTEND_PID=$!
echo "$FRONTEND_PID" > "$FRONTEND_PID_FILE"
echo "Frontend started (PID $FRONTEND_PID)."

echo ""
echo "Both servers are running. Press Ctrl+C to stop."

wait $BACKEND_PID $FRONTEND_PID
