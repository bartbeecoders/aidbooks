#!/usr/bin/env bash
# Real round-trip against mold-service /v1/generate. Requires the
# upstream mold serve to be reachable and a model to be pulled (defaults
# to `flux2-klein:q8`, override via MOLD_TEST_MODEL).
#
# Writes the decoded image to a temp file and prints its path so you
# can inspect it manually.
set -euo pipefail

BASE_URL="${MOLD_SERVICE_URL:-http://127.0.0.1:7681}"
PROMPT="${MOLD_TEST_PROMPT:-a wide cinematic painting of a cat riding a motorcycle through neon-lit streets}"
MODEL="${MOLD_TEST_MODEL:-}"
IS_SHORT="${MOLD_TEST_IS_SHORT:-false}"
OUT="${MOLD_TEST_OUT:-$(mktemp -t mold-service.XXXXXX.png)}"

EXTRA_ARGS=()
if [[ -n "${MOLD_SERVICE_API_KEY:-}" ]]; then
    EXTRA_ARGS+=("-H" "X-Api-Key: $MOLD_SERVICE_API_KEY")
fi

# Build the JSON body. Use python or jq if available; otherwise hand-roll
# a minimal payload — we control the inputs, so escaping risk is low.
build_body_python() {
    python3 - <<PY
import json, os
print(json.dumps({
    "prompt": os.environ["PROMPT"],
    "is_short": os.environ["IS_SHORT"].lower() == "true",
    **({"model": os.environ["MODEL"]} if os.environ.get("MODEL") else {}),
}))
PY
}
build_body_jq() {
    jq -n \
        --arg p "$PROMPT" \
        --arg m "$MODEL" \
        --argjson s "$([[ "$IS_SHORT" == "true" ]] && echo true || echo false)" \
        '{prompt:$p, is_short:$s} + (if $m == "" then {} else {model:$m} end)'
}
export PROMPT IS_SHORT MODEL
if command -v python3 >/dev/null 2>&1; then
    BODY="$(build_body_python)"
elif command -v jq >/dev/null 2>&1; then
    BODY="$(build_body_jq)"
else
    echo "Need python3 or jq to build JSON body" >&2
    exit 1
fi

echo "POST $BASE_URL/v1/generate"
echo "  body: $BODY"

RESP_FILE="$(mktemp -t mold-service.XXXXXX.json)"
HTTP_CODE="$(curl -sS --max-time 600 -o "$RESP_FILE" -w "%{http_code}" \
    "${EXTRA_ARGS[@]}" \
    -H "Content-Type: application/json" \
    --data "$BODY" \
    "$BASE_URL/v1/generate")"

if [[ "$HTTP_CODE" != "200" ]]; then
    echo "FAIL: HTTP $HTTP_CODE" >&2
    cat "$RESP_FILE" >&2
    echo >&2
    exit 1
fi

# Pull image_base64, content_type, width, height. Prefer python3 for
# robust JSON decoding; fall back to jq if present.
decode_python() {
    python3 - "$RESP_FILE" "$OUT" <<'PY'
import base64, json, sys
resp = json.load(open(sys.argv[1]))
data = base64.b64decode(resp["image_base64"])
open(sys.argv[2], "wb").write(data)
print(f"width={resp['width']} height={resp['height']} model={resp['model']} steps={resp['steps']} content_type={resp['content_type']} seed_used={resp.get('seed_used')} bytes={len(data)}")
PY
}

if command -v python3 >/dev/null 2>&1; then
    SUMMARY="$(decode_python)"
elif command -v jq >/dev/null 2>&1; then
    jq -r '.image_base64' "$RESP_FILE" | base64 -d > "$OUT"
    SUMMARY="$(jq -r '"width=\(.width) height=\(.height) model=\(.model) steps=\(.steps) content_type=\(.content_type) seed_used=\(.seed_used)"' "$RESP_FILE")"
else
    echo "Need python3 or jq to decode the response" >&2
    exit 1
fi

SIZE="$(wc -c < "$OUT")"
echo "  $SUMMARY file=$OUT file_size=$SIZE bytes"

if (( SIZE < 1024 )); then
    echo "FAIL: image is suspiciously small (<1KB)" >&2
    exit 1
fi
echo "ok"
