#!/usr/bin/env bash
# Drop every loaded model from the upstream mold instance's GPU cache.
# Server-wide — every client of that mold reloads from disk on the
# next request.
set -euo pipefail

BASE_URL="${MOLD_SERVICE_URL:-http://127.0.0.1:7681}"
EXTRA_ARGS=()
if [[ -n "${MOLD_SERVICE_API_KEY:-}" ]]; then
    EXTRA_ARGS+=("-H" "X-Api-Key: $MOLD_SERVICE_API_KEY")
fi

echo "DELETE $BASE_URL/v1/models/unload"
RESP="$(curl -fsS --max-time 30 -X DELETE \
    "${EXTRA_ARGS[@]}" \
    "$BASE_URL/v1/models/unload")"
echo "  $RESP"
echo "ok"
