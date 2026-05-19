#!/usr/bin/env bash
# Pull a model on the upstream mold instance via mold-service. Slow —
# typically minutes. Override the model via MOLD_TEST_MODEL.
set -euo pipefail

BASE_URL="${MOLD_SERVICE_URL:-http://127.0.0.1:7681}"
MODEL="${MOLD_TEST_MODEL:-flux2-klein:q8}"
EXTRA_ARGS=()
if [[ -n "${MOLD_SERVICE_API_KEY:-}" ]]; then
    EXTRA_ARGS+=("-H" "X-Api-Key: $MOLD_SERVICE_API_KEY")
fi

echo "POST $BASE_URL/v1/models/pull (model=$MODEL)"
RESP="$(curl -fsS --max-time 3600 \
    "${EXTRA_ARGS[@]}" \
    -H "Content-Type: application/json" \
    --data "{\"model\":\"$MODEL\"}" \
    "$BASE_URL/v1/models/pull")"
echo "  $RESP"
echo "ok"
