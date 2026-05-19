#!/usr/bin/env bash
# Probe mold-service /healthz. Fails when the service is unreachable or
# the response payload doesn't include `"status":"ok"`. Reports whether
# the upstream mold serve is reachable too, but does not fail on that —
# the service is still up if mold is down.
set -euo pipefail

BASE_URL="${MOLD_SERVICE_URL:-http://127.0.0.1:7681}"
EXTRA_ARGS=()
if [[ -n "${MOLD_SERVICE_API_KEY:-}" ]]; then
    EXTRA_ARGS+=("-H" "X-Api-Key: $MOLD_SERVICE_API_KEY")
fi

echo "GET $BASE_URL/healthz"
RESP="$(curl -fsS --max-time 5 "${EXTRA_ARGS[@]}" "$BASE_URL/healthz")"
echo "  $RESP"

# Tolerant JSON check — works without jq.
if ! grep -q '"status":"ok"' <<<"$RESP"; then
    echo "FAIL: healthz did not report ok" >&2
    exit 1
fi
if grep -q '"upstream_reachable":false' <<<"$RESP"; then
    echo "  note: upstream mold serve is NOT reachable (image-gen calls will 502)"
fi
echo "ok"
