#!/usr/bin/env bash
# All-in-one smoke runner for mold-service. By default it boots the
# service itself (`cargo run --release -p mold-service`) on an ephemeral
# port, waits for /healthz, then runs:
#
#   - health.sh       — basic liveness + upstream check
#   - generate.sh     — defaults + a real image-gen round-trip
#   - pull.sh         — only if MOLD_TEST_PULL=1 (slow)
#   - unload.sh       — only if MOLD_TEST_UNLOAD=1
#
# Skip the service boot (and reuse a running one) with MOLD_SKIP_BOOT=1.
# Skip the live generate (e.g. when no GPU is reachable) with
# MOLD_SKIP_GENERATE=1 — Rust unit/integration tests still cover the
# code paths without a GPU via `cargo test`.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT_DIR="$DIR/scripts"

BIND="${MOLD_SERVICE_BIND:-127.0.0.1}"
PORT="${MOLD_SERVICE_PORT:-7681}"
BASE_URL="${MOLD_SERVICE_URL:-http://${BIND}:${PORT}}"
export MOLD_SERVICE_URL="$BASE_URL"

cyan()  { printf "\033[36m%s\033[0m\n" "$*"; }
green() { printf "\033[32m%s\033[0m\n" "$*"; }
red()   { printf "\033[31m%s\033[0m\n" "$*" >&2; }

cleanup() {
    if [[ -n "${SERVICE_PID:-}" ]] && kill -0 "$SERVICE_PID" 2>/dev/null; then
        cyan "Stopping mold-service (PID $SERVICE_PID)..."
        kill "$SERVICE_PID" 2>/dev/null || true
        wait "$SERVICE_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

if [[ "${MOLD_SKIP_BOOT:-0}" != "1" ]]; then
    cyan "Booting mold-service on $BIND:$PORT (release build, foreground tail)..."
    LOG_FILE="$(mktemp -t mold-service.XXXXXX.log)"
    cyan "  logs: $LOG_FILE"
    (
        cd "$DIR"
        MOLD_SERVICE_BIND="$BIND" MOLD_SERVICE_PORT="$PORT" \
            cargo run --release --quiet
    ) >"$LOG_FILE" 2>&1 &
    SERVICE_PID=$!

    cyan "Waiting for $BASE_URL/healthz ..."
    WAITED=0
    until curl -fsS --max-time 1 "$BASE_URL/healthz" >/dev/null 2>&1; do
        if ! kill -0 "$SERVICE_PID" 2>/dev/null; then
            red "mold-service exited before becoming ready — see $LOG_FILE"
            tail -n 40 "$LOG_FILE" >&2 || true
            exit 1
        fi
        if (( WAITED >= 120 )); then
            red "mold-service did not become ready within 120s — see $LOG_FILE"
            tail -n 40 "$LOG_FILE" >&2 || true
            exit 1
        fi
        sleep 1
        WAITED=$(( WAITED + 1 ))
    done
    green "  ready (after ${WAITED}s)"
fi

cyan "=== health.sh ==="
"$SCRIPT_DIR/health.sh"

if [[ "${MOLD_SKIP_GENERATE:-0}" != "1" ]]; then
    cyan "=== generate.sh ==="
    "$SCRIPT_DIR/generate.sh"
else
    cyan "Skipping generate.sh (MOLD_SKIP_GENERATE=1)"
fi

if [[ "${MOLD_TEST_PULL:-0}" == "1" ]]; then
    cyan "=== pull.sh ==="
    "$SCRIPT_DIR/pull.sh"
fi

if [[ "${MOLD_TEST_UNLOAD:-0}" == "1" ]]; then
    cyan "=== unload.sh ==="
    "$SCRIPT_DIR/unload.sh"
fi

green "All smoke tests passed."
