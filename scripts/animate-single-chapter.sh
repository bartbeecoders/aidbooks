#!/usr/bin/env bash
#
# Single-chapter standalone demo of the animation renderer.
#
# Unlike scripts/animate-demo.sh, this script does NOT touch the backend
# at all — no API server, no database, no LLM, no audiobook pipeline.
# It exercises only the Node renderer in backend/render/ with a hand-
# crafted SceneSpec, so you can judge the animation tool's output
# quality in isolation before deciding whether to wire it into the full
# feature.
#
# What it does:
#   1. Verifies ffmpeg / ffprobe / node are on PATH.
#   2. Builds backend/render/dist/cli.js if missing.
#   3. Generates a ~15s sine-sweep WAV (24 kHz mono — matches the TTS
#      pipeline's real output rate).
#   4. Generates a synthetic 500-bucket waveform.json so the
#      WaveformPulse component has something to react to.
#   5. Builds a one-chapter SceneSpec (title → 2 paragraphs → outro)
#      that mirrors what the Phase B planner produces.
#   6. Pipes the spec to `node backend/render/dist/cli.js` and tails
#      the NDJSON progress events.
#   7. Probes the resulting MP4 (1920x1080 H.264 + AAC, duration drift
#      under 250 ms).
#   8. Opens the MP4 via xdg-open / open unless --no-open was passed.
#
# Usage:
#   scripts/animate-single-chapter.sh                  # default
#   scripts/animate-single-chapter.sh --theme parchment
#   scripts/animate-single-chapter.sh --duration 8     # seconds
#   scripts/animate-single-chapter.sh --no-open
#   scripts/animate-single-chapter.sh --keep           # leave WAV/peaks
#
# Output lands in backend/render/test/output/single-chapter/.
# Exits 0 on success, non-zero on any failure.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RENDER_DIR="$ROOT/backend/render"
OUT_DIR="$RENDER_DIR/test/output/single-chapter"

THEME="library"
DURATION_S=15
OPEN_RESULT=true
KEEP_INPUTS=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --theme)        THEME="$2"; shift 2 ;;
        --theme=*)      THEME="${1#*=}"; shift ;;
        --duration)     DURATION_S="$2"; shift 2 ;;
        --duration=*)   DURATION_S="${1#*=}"; shift ;;
        --no-open)      OPEN_RESULT=false; shift ;;
        --keep)         KEEP_INPUTS=true; shift ;;
        -h|--help)
            sed -n '3,33p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *)
            echo "Unknown arg: $1" >&2
            echo "Try --help" >&2
            exit 2 ;;
    esac
done

case "$THEME" in
    library|parchment|minimal) ;;
    *) echo "FAIL: --theme must be one of library, parchment, minimal (got '$THEME')" >&2; exit 2 ;;
esac

if ! [[ "$DURATION_S" =~ ^[0-9]+$ ]] || (( DURATION_S < 4 )); then
    echo "FAIL: --duration must be a positive integer >= 4 (got '$DURATION_S')" >&2
    exit 2
fi

step() { echo; echo "==> $*"; }
ok()   { echo "    ✓ $*"; }
fail() { echo "    ✗ $*" >&2; exit 1; }

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        fail "required command '$1' not found on PATH"
    fi
}
require ffmpeg
require ffprobe
require node

# --- 1. Build renderer if needed ------------------------------------------

if [[ ! -f "$RENDER_DIR/dist/cli.js" ]]; then
    step "Building backend/render (one-time)"
    (cd "$RENDER_DIR" && npm install --no-audit --no-fund && npm run build)
    ok "renderer built at $RENDER_DIR/dist/cli.js"
else
    ok "renderer build present"
fi

# --- 2. Prep output dir ---------------------------------------------------

step "Preparing $OUT_DIR"
rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"
WAV_PATH="$OUT_DIR/fixture.wav"
PEAKS_PATH="$OUT_DIR/fixture.waveform.json"
SPEC_PATH="$OUT_DIR/spec.json"
NDJSON_PATH="$OUT_DIR/progress.ndjson"
MP4_PATH="$OUT_DIR/ch-1.video.mp4"
ok "ready"

# --- 3. Generate WAV ------------------------------------------------------

step "Generating ${DURATION_S}s sine WAV"
ffmpeg -y \
    -f lavfi \
    -i "sine=frequency=440:duration=$DURATION_S" \
    -ar 24000 -ac 1 \
    "$WAV_PATH" \
    >/dev/null 2>&1 \
    || fail "ffmpeg WAV generation failed"
ok "wrote $WAV_PATH"

# --- 4. Generate synthetic waveform.json ---------------------------------

step "Generating synthetic waveform.json (500 buckets)"
node --input-type=module -e '
const { writeFileSync } = await import("node:fs");
const out = process.argv[1];
const buckets = 500;
const peaks = Array.from({ length: buckets }, (_, i) => {
  const t = i / buckets;
  return Math.abs(Math.sin(Math.PI * t * 4)) * 0.8 + 0.05;
});
writeFileSync(out, JSON.stringify({ sample_rate_hz: 24000, buckets, peaks }));
' "$PEAKS_PATH"
ok "wrote $PEAKS_PATH"

# --- 5. Build SceneSpec ---------------------------------------------------

step "Building SceneSpec (one chapter, title + 2 paragraphs + outro)"
DURATION_MS=$(( DURATION_S * 1000 ))
TITLE_END=$(( DURATION_MS < 8000 ? DURATION_MS / 4 : 4000 ))
OUTRO_DUR=$(( DURATION_MS < 8000 ? DURATION_MS / 5 : 3000 ))
PARA_START=$TITLE_END
PARA_END=$(( DURATION_MS - OUTRO_DUR ))
PARA_MID=$(( PARA_START + (PARA_END - PARA_START) / 2 ))

node --input-type=module -e '
const { writeFileSync } = await import("node:fs");
const [out, wav, peaks, mp4, theme, durationMs, titleEnd, paraMid, paraEnd] = process.argv.slice(1);
const ms = Number;
const spec = {
  version: 1,
  chapter: {
    number: 1,
    title: "The Trust Stack",
    duration_ms: ms(durationMs),
  },
  audio: { wav, peaks },
  theme: { preset: theme, primary: null, accent: null },
  background: { kind: "color", color: "#0F172A" },
  scenes: [
    {
      kind: "title",
      start_ms: 0,
      end_ms: ms(titleEnd),
      title: "The Trust Stack",
      subtitle: "Chapter 1",
    },
    {
      kind: "paragraph",
      start_ms: ms(titleEnd),
      end_ms: ms(paraMid),
      text: "Trust is the substrate of every transaction. Strip it away and even the simplest trade collapses into bargaining about reliability rather than price.",
      tile: null,
      highlight: "karaoke",
    },
    {
      kind: "paragraph",
      start_ms: ms(paraMid),
      end_ms: ms(paraEnd),
      text: "What feels like cynicism in a market is usually unmet expectations. The cost of switching context is exactly the cost of rebuilding that trust.",
      tile: null,
      highlight: "karaoke",
    },
    {
      kind: "outro",
      start_ms: ms(paraEnd),
      end_ms: ms(durationMs),
      title: "Continue listening",
      subtitle: "listenai.app",
    },
  ],
  captions: null,
  output: {
    mp4,
    width: 1920,
    height: 1080,
    fps: 30,
  },
};
writeFileSync(out, JSON.stringify(spec, null, 2));
' "$SPEC_PATH" "$WAV_PATH" "$PEAKS_PATH" "$MP4_PATH" "$THEME" "$DURATION_MS" "$TITLE_END" "$PARA_MID" "$PARA_END"
ok "wrote $SPEC_PATH (theme=$THEME, duration=${DURATION_MS}ms)"

# --- 6. Run the renderer --------------------------------------------------

step "Running renderer (this boots Chromium — may take 10-30s)"
START_TS=$(date +%s)
set +e
# Run from RENDER_DIR so Vite (used internally by Revideo) resolves
# node_modules from backend/render/ rather than the user's shell CWD.
( cd "$RENDER_DIR" && node dist/cli.js < "$SPEC_PATH" ) \
    | tee "$NDJSON_PATH" \
    | while IFS= read -r line; do
        # Pretty-print the NDJSON stream so the user sees progress.
        TYPE=$(echo "$line" | node -e 'let s=""; process.stdin.on("data",c=>s+=c).on("end",()=>{ try { process.stdout.write(JSON.parse(s).type ?? "") } catch {} })' 2>/dev/null || echo "")
        case "$TYPE" in
            started)  echo "    [renderer] started" ;;
            frame)    : ;;
            encoding) echo "    [renderer] encoding…" ;;
            done)     echo "    [renderer] done" ;;
            error)
                MSG=$(echo "$line" | node -e 'let s=""; process.stdin.on("data",c=>s+=c).on("end",()=>{ try { process.stdout.write(JSON.parse(s).message ?? "") } catch {} })' 2>/dev/null)
                echo "    [renderer] error: $MSG" >&2
                ;;
        esac
    done
RC=${PIPESTATUS[0]}
set -e
END_TS=$(date +%s)
ELAPSED=$(( END_TS - START_TS ))

if [[ $RC -ne 0 ]]; then
    echo "    last NDJSON lines:"
    tail -n 5 "$NDJSON_PATH" | sed 's/^/      /'
    fail "renderer exited $RC (see $NDJSON_PATH for full progress log)"
fi
ok "rendered in ${ELAPSED}s"

# --- 7. Probe output ------------------------------------------------------

step "Verifying $MP4_PATH"
if [[ ! -f "$MP4_PATH" ]]; then
    fail "renderer reported success but $MP4_PATH is missing"
fi

PROBE=$(ffprobe -v error \
    -show_entries 'stream=codec_name,codec_type,width,height,r_frame_rate,sample_rate:format=duration' \
    -of json \
    "$MP4_PATH")

V_CODEC=$(echo "$PROBE" | node -e 'let s=""; process.stdin.on("data",c=>s+=c).on("end",()=>{ const p=JSON.parse(s); const v=p.streams.find(x=>x.codec_type==="video"); process.stdout.write(v?.codec_name ?? "") })')
V_W=$(echo     "$PROBE" | node -e 'let s=""; process.stdin.on("data",c=>s+=c).on("end",()=>{ const p=JSON.parse(s); const v=p.streams.find(x=>x.codec_type==="video"); process.stdout.write(String(v?.width ?? 0)) })')
V_H=$(echo     "$PROBE" | node -e 'let s=""; process.stdin.on("data",c=>s+=c).on("end",()=>{ const p=JSON.parse(s); const v=p.streams.find(x=>x.codec_type==="video"); process.stdout.write(String(v?.height ?? 0)) })')
V_FPS=$(echo   "$PROBE" | node -e 'let s=""; process.stdin.on("data",c=>s+=c).on("end",()=>{ const p=JSON.parse(s); const v=p.streams.find(x=>x.codec_type==="video"); const [n,d]=(v?.r_frame_rate ?? "0/1").split("/").map(Number); process.stdout.write(String(Math.round(n/(d||1)))) })')
A_CODEC=$(echo "$PROBE" | node -e 'let s=""; process.stdin.on("data",c=>s+=c).on("end",()=>{ const p=JSON.parse(s); const a=p.streams.find(x=>x.codec_type==="audio"); process.stdout.write(a?.codec_name ?? "") })')
DUR_S=$(echo   "$PROBE" | node -e 'let s=""; process.stdin.on("data",c=>s+=c).on("end",()=>{ const p=JSON.parse(s); process.stdout.write(p.format?.duration ?? "0") })')
SIZE_BYTES=$(stat -c '%s' "$MP4_PATH" 2>/dev/null || stat -f '%z' "$MP4_PATH")
SIZE_HUMAN=$(numfmt --to=iec --suffix=B "$SIZE_BYTES" 2>/dev/null || echo "${SIZE_BYTES}B")

echo "    video : ${V_W}x${V_H}@${V_FPS}fps ${V_CODEC}"
echo "    audio : ${A_CODEC}"
echo "    duration: ${DUR_S}s (expected ~${DURATION_S}s)"
echo "    size  : $SIZE_HUMAN"

[[ "$V_W" == "1920" && "$V_H" == "1080" ]] || fail "expected 1920x1080, got ${V_W}x${V_H}"
[[ "$V_CODEC" == "h264" ]] || fail "expected h264 video, got $V_CODEC"
[[ "$A_CODEC" == "aac" ]] || fail "expected aac audio, got $A_CODEC"

# Duration drift check (within ±300ms of spec). Use awk for float math.
DRIFT_MS=$(awk -v d="$DUR_S" -v want="$DURATION_S" 'BEGIN { diff = (d - want) * 1000; if (diff < 0) diff = -diff; printf "%d", diff }')
if (( DRIFT_MS > 300 )); then
    fail "duration ${DUR_S}s drifted ${DRIFT_MS}ms from expected ${DURATION_S}s"
fi
ok "verified (drift ${DRIFT_MS}ms)"

# --- 8. Open the result ---------------------------------------------------

if $OPEN_RESULT; then
    if command -v xdg-open >/dev/null 2>&1; then
        ( xdg-open "$MP4_PATH" >/dev/null 2>&1 & )
        ok "opened with xdg-open"
    elif command -v open >/dev/null 2>&1; then
        ( open "$MP4_PATH" >/dev/null 2>&1 & )
        ok "opened with open"
    else
        echo "    (no xdg-open / open found — preview manually)"
    fi
fi

# --- 9. Cleanup -----------------------------------------------------------

if ! $KEEP_INPUTS; then
    rm -f "$WAV_PATH" "$PEAKS_PATH"
fi

echo
echo "Done. The animated chapter MP4 is at:"
echo "  $MP4_PATH"
echo
echo "Inputs / progress log under:"
echo "  $OUT_DIR"
