#!/usr/bin/env bash
#
# End-to-end demo of the animation feature.
#
# What it does:
#   1. Verifies prerequisites (ffmpeg, jq, curl, node) and that the backend
#      is reachable at $LISTENAI_API_URL (default http://127.0.0.1:8787).
#   2. Builds backend/render if dist/cli.js is missing.
#   3. Logs in as the demo admin (LISTENAI_DEV_SEED=true must be set).
#   4. Creates a tiny audiobook with auto_pipeline.{chapters, audio} so the
#      backend writes chapters + narrates them on its own — works in mock
#      mode without API keys.
#   5. Polls until status=audio_ready (chapters narrated).
#   6. POSTs /animate and polls /jobs until every animate_chapter is
#      `completed`.
#   7. Prints the path to ch-1.video.mp4 and tries to open it via xdg-open
#      (Linux) or `open` (macOS).
#
# Usage:
#   scripts/animate-demo.sh                       # default: real renderer
#   scripts/animate-demo.sh --mock                # ffmpeg-only fallback
#   scripts/animate-demo.sh --keep-book           # don't delete the book
#   scripts/animate-demo.sh --topic "..."         # override the LLM topic
#   scripts/animate-demo.sh --no-open             # skip xdg-open / open
#
# Prereqs the script can't fix itself:
#   - Backend running with LISTENAI_DEV_SEED=true
#   - LISTENAI_ANIMATE_RENDERER_CMD pointing at backend/render/dist/cli.js
#     (or LISTENAI_ANIMATE_MOCK=true; the --mock flag sets the env var
#     for *this* script only, but the backend ignores that — set it in
#     the backend's environment).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RENDER_DIR="$ROOT/backend/render"
API_BASE="${LISTENAI_API_URL:-http://127.0.0.1:8787}"
EMAIL="${LISTENAI_DEMO_EMAIL:-demo@listenai.local}"
PASSWORD="${LISTENAI_DEMO_PASSWORD:-demo}"

TOPIC="A short reflection on trust, attention, and the cost of switching context — three paragraphs, intentional pauses, friendly tone."
KEEP_BOOK=false
OPEN_RESULT=true
MOCK_HINT=false   # purely informational; the backend reads its own env

while [[ $# -gt 0 ]]; do
    case "$1" in
        --topic)        TOPIC="$2"; shift 2 ;;
        --topic=*)      TOPIC="${1#*=}"; shift ;;
        --keep-book)    KEEP_BOOK=true; shift ;;
        --no-open)      OPEN_RESULT=false; shift ;;
        --mock)         MOCK_HINT=true; shift ;;
        -h|--help)
            sed -n '3,30p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *)
            echo "Unknown arg: $1" >&2
            echo "Try --help" >&2
            exit 2 ;;
    esac
done

# --- 1. Prereq checks ------------------------------------------------------

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "FAIL: required command '$1' not found on PATH" >&2
        exit 1
    fi
}
require curl
require jq
require ffmpeg
require ffprobe
require node

step() { echo; echo "==> $*"; }
ok()   { echo "    ✓ $*"; }
fail() { echo "    ✗ $*" >&2; exit 1; }

step "Checking backend at $API_BASE"
if ! curl -fsS --max-time 3 "$API_BASE/health" >/dev/null; then
    fail "backend not reachable. Start it with: just dev-backend (with LISTENAI_DEV_SEED=true)"
fi
ok "backend up"

# --- 2. Build the renderer if needed --------------------------------------

if [[ ! -f "$RENDER_DIR/dist/cli.js" ]]; then
    step "Building backend/render (one-time)"
    (cd "$RENDER_DIR" && npm install --no-audit --no-fund && npm run build)
    ok "renderer built at $RENDER_DIR/dist/cli.js"
else
    ok "renderer build present"
fi

if $MOCK_HINT; then
    echo
    echo "    Note: --mock only affects the local fixture script; the backend"
    echo "    follows LISTENAI_ANIMATE_MOCK from its own environment. Set it"
    echo "    in .env and restart the backend if you want mock-mode renders."
fi

# --- 3. Login -------------------------------------------------------------

step "Logging in as $EMAIL"
LOGIN_BODY=$(jq -n --arg email "$EMAIL" --arg password "$PASSWORD" \
    '{email: $email, password: $password}')
LOGIN_RESP=$(curl -fsS -X POST "$API_BASE/auth/login" \
    -H 'content-type: application/json' \
    -d "$LOGIN_BODY") || fail "login failed (is LISTENAI_DEV_SEED=true?)"
ACCESS_TOKEN=$(echo "$LOGIN_RESP" | jq -r '.access_token')
[[ -n "$ACCESS_TOKEN" && "$ACCESS_TOKEN" != "null" ]] || fail "no access_token in login response"
ok "logged in"

api() {
    local method="$1"; local path="$2"; shift 2
    curl -fsS -X "$method" "$API_BASE$path" \
        -H "authorization: Bearer $ACCESS_TOKEN" \
        -H 'content-type: application/json' \
        "$@"
}

# --- 4. Create the audiobook ----------------------------------------------

step "Creating audiobook"
CREATE_BODY=$(jq -n --arg topic "$TOPIC" '{
    topic: $topic,
    length: "short",
    language: "en",
    auto_pipeline: { chapters: true, cover: false, audio: true }
}')
CREATE_RESP=$(api POST /audiobook -d "$CREATE_BODY") \
    || fail "create audiobook failed"
BOOK_ID=$(echo "$CREATE_RESP" | jq -r '.id')
[[ -n "$BOOK_ID" && "$BOOK_ID" != "null" ]] || fail "no id in create response"
ok "audiobook id: $BOOK_ID"

# --- 5. Wait for audio_ready ----------------------------------------------

step "Waiting for chapters + narration (status=audio_ready)"
DEADLINE=$(( $(date +%s) + 600 ))   # cap at 10 min in case of a real LLM run
LAST_STATUS=""
while true; do
    DETAIL=$(api GET "/audiobook/$BOOK_ID") || fail "fetch audiobook failed"
    STATUS=$(echo "$DETAIL" | jq -r '.status')
    if [[ "$STATUS" != "$LAST_STATUS" ]]; then
        echo "    status: $STATUS"
        LAST_STATUS="$STATUS"
    fi
    if [[ "$STATUS" == "audio_ready" ]]; then
        ok "audio_ready"
        break
    fi
    if [[ "$STATUS" == "failed" ]]; then
        echo "    book status went to failed — recent jobs:"
        api GET "/audiobook/$BOOK_ID/jobs" | jq -r '.jobs[] | "      \(.kind) \(.status) \(.last_error // "")"'
        fail "audiobook failed"
    fi
    if (( $(date +%s) >= DEADLINE )); then
        fail "timed out waiting for audio_ready (status=$STATUS)"
    fi
    sleep 2
done

# --- 6. Animate -----------------------------------------------------------

step "Triggering animation"
api POST "/audiobook/$BOOK_ID/animate?language=en&theme=library" >/dev/null \
    || fail "POST /animate failed"
ok "animate enqueued"

step "Waiting for every chapter to render"
CHAPTER_COUNT=$(echo "$DETAIL" | jq '.chapters | length')
DEADLINE=$(( $(date +%s) + 3600 ))
ANIMATE_START=$(date +%s)
PREV_LINE=""

# Sum chapter audio durations once — gives the user an honest rough ETA
# at first paint. ~5× realtime is what we observe on a typical CPU with
# Revideo's per-frame canvas paint; the actual figure depends on host
# CPU + Chromium version, but it's the right order of magnitude.
TOTAL_AUDIO_S=$(echo "$DETAIL" | jq '[.chapters[].duration_ms // 0] | add / 1000 | floor')
SLOTS=$(( CHAPTER_COUNT < 2 ? CHAPTER_COUNT : 2 ))
if [[ "$TOTAL_AUDIO_S" -gt 0 ]]; then
    EST_S=$(( (TOTAL_AUDIO_S * 5) / SLOTS ))
    echo "    audio total: ${TOTAL_AUDIO_S}s · pool: ${SLOTS}-wide · estimate: ~$((EST_S / 60))m $((EST_S % 60))s at 5× realtime"
fi

while true; do
    JOBS=$(api GET "/audiobook/$BOOK_ID/jobs") || fail "fetch jobs failed"
    DONE=$(echo "$JOBS" | jq '[.jobs[] | select(.kind == "animate_chapter" and .status == "completed")] | length')
    DEAD=$(echo "$JOBS" | jq '[.jobs[] | select(.kind == "animate_chapter" and (.status == "dead" or .status == "failed"))] | length')
    if [[ "$DEAD" -gt 0 ]]; then
        echo "    one or more chapters failed — recent errors:"
        echo "$JOBS" | jq -r '.jobs[] | select(.kind == "animate_chapter" and .last_error) | "      ch\(.chapter_number // "?"): \(.last_error)"'
        fail "animation failed"
    fi
    if [[ "$DONE" == "$CHAPTER_COUNT" ]]; then
        echo "    [$(printf '%0.s█' $(seq 1 20))] $CHAPTER_COUNT/$CHAPTER_COUNT chapters"
        ok "all $CHAPTER_COUNT chapters rendered"
        break
    fi

    # Build a one-line snapshot per chapter (sorted by number) so the
    # user sees both the running cells advancing and the queued ones
    # waiting. We pin to one line per tick rather than scrolling, with a
    # leading ANSI cursor-up jump after the first paint so the output
    # stays compact. Falls back gracefully when stdout is a pipe.
    SNAPSHOT=$(echo "$JOBS" | jq -r '
        [.jobs[] | select(.kind == "animate_chapter")]
        | group_by(.chapter_number)
        | map(max_by(.queued_at))
        | sort_by(.chapter_number)
        | map(
            ("ch" + (.chapter_number | tostring | (if length < 2 then "0" + . else . end)))
            + " " + (
                if .status == "completed" then "✓ ready 100%"
                elif .status == "running" then "→ " + (((.progress_pct * 100) | floor) | tostring) + "%"
                elif .status == "queued" then "· wait     "
                else .status end
            )
          )
        | join("  ")
    ')
    ELAPSED=$(( $(date +%s) - ANIMATE_START ))
    LINE=$(printf "    [%dm%02ds] %s" $((ELAPSED / 60)) $((ELAPSED % 60)) "$SNAPSHOT")
    if [[ "$LINE" != "$PREV_LINE" ]]; then
        # \r + clear line so the bar updates cleanly when the script is
        # attached to a TTY. When tee'd to a file the carriage return is
        # harmless and each tick still appears as its own line break in
        # most pagers.
        if [[ -t 1 ]]; then
            printf '\r\033[K%s' "$LINE"
        else
            echo "$LINE"
        fi
        PREV_LINE="$LINE"
    fi

    if (( $(date +%s) >= DEADLINE )); then
        [[ -t 1 ]] && echo
        fail "timed out waiting for animation (rendered $DONE / $CHAPTER_COUNT)"
    fi
    sleep 2
done
[[ -t 1 ]] && echo

# --- 7. Inspect the result ------------------------------------------------

step "Locating output"
STORAGE_PATH="${LISTENAI_STORAGE_PATH:-$ROOT/storage/audio}"
OUT_DIR="$STORAGE_PATH/$BOOK_ID/en"
FIRST_MP4="$OUT_DIR/ch-1.video.mp4"
if [[ ! -f "$FIRST_MP4" ]]; then
    fail "expected $FIRST_MP4 to exist but it doesn't"
fi
ok "wrote $(ls "$OUT_DIR"/ch-*.video.mp4 | wc -l) chapter MP4(s) under $OUT_DIR"

DURATION_S=$(ffprobe -v error -show_entries format=duration -of default=nokey=1:noprint_wrappers=1 "$FIRST_MP4")
SIZE_BYTES=$(stat -c '%s' "$FIRST_MP4" 2>/dev/null || stat -f '%z' "$FIRST_MP4")
echo "    ch-1.video.mp4  ${DURATION_S}s  $(numfmt --to=iec --suffix=B "$SIZE_BYTES" 2>/dev/null || echo "${SIZE_BYTES}B")"

if $OPEN_RESULT; then
    if command -v xdg-open >/dev/null 2>&1; then
        ( xdg-open "$FIRST_MP4" >/dev/null 2>&1 & )
        ok "opened with xdg-open"
    elif command -v open >/dev/null 2>&1; then
        ( open "$FIRST_MP4" >/dev/null 2>&1 & )
        ok "opened with open"
    else
        echo "    (no xdg-open / open found — preview manually: $FIRST_MP4)"
    fi
fi

# --- 8. Cleanup -----------------------------------------------------------

if ! $KEEP_BOOK; then
    step "Cleaning up demo audiobook"
    api DELETE "/audiobook/$BOOK_ID" >/dev/null && ok "deleted $BOOK_ID" \
        || echo "    (delete failed, leave it manually)"
else
    echo
    echo "Audiobook left in place: $BOOK_ID (--keep-book)"
fi

echo
echo "Done. The first chapter MP4 is at:"
echo "  $FIRST_MP4"
