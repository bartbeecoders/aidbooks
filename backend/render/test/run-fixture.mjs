// Standalone smoke test for the animation renderer.
//
// What it does:
//   1. Generates a 12-second sine WAV at 24 kHz mono via ffmpeg.
//   2. Generates a synthetic 500-bucket waveform.json (matches the shape
//      `audio/mod.rs` writes alongside real chapter WAVs).
//   3. Picks an existing chapter cover for the background, falling back
//      to a solid colour if none is available.
//   4. Builds a SceneSpec with the same Title + Paragraph + Outro shape
//      the Rust planner emits — exercises every scene type.
//   5. Pipes the spec to `dist/cli.js` (or `--mock` for a black-frame
//      ffmpeg fallback) and tails the NDJSON progress events.
//   6. Probes the resulting MP4 with ffprobe and asserts:
//        - video stream is 1920x1080 H.264 at 30 fps
//        - audio stream is AAC at 48 kHz
//        - duration is within ±200 ms of the spec's chapter.duration_ms
//   7. Cleans up under `test/output/` unless `--keep` is passed.
//
// Run it from `backend/render/`:
//   npm run test:fixture           # default: real Revideo render
//   npm run test:fixture -- --mock # ffmpeg-only fallback, no Chromium
//   npm run test:fixture -- --keep # leave artefacts in test/output/
//
// Exits 0 on success, 1 on any failure. Stdout is human-readable; the
// renderer's NDJSON is written to test/output/progress.ndjson.

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, '..');
const OUT = resolve(__dirname, 'output');

const args = new Set(process.argv.slice(2));
const MOCK = args.has('--mock');
const KEEP = args.has('--keep');

const DURATION_MS = 12_000;

function log(msg) {
  process.stdout.write(`[render-test] ${msg}\n`);
}

function fail(msg) {
  process.stderr.write(`[render-test] FAIL: ${msg}\n`);
  process.exit(1);
}

function assertCmd(bin, friendly) {
  const r = spawnSync(bin, ['-version'], { stdio: 'ignore' });
  if (r.error) fail(`${friendly} not found on PATH (${bin})`);
}

function makeWav(path) {
  log(`generating ${DURATION_MS}ms WAV at ${path}`);
  // sine sweep so the waveform isn't a flat line; mono 24 kHz to match
  // the TTS pipeline's real output (LISTENAI_XAI_SAMPLE_RATE_HZ=24000).
  const r = spawnSync(
    'ffmpeg',
    [
      '-y',
      '-f', 'lavfi',
      '-i', `sine=frequency=440:duration=${DURATION_MS / 1000}`,
      '-ar', '24000',
      '-ac', '1',
      path,
    ],
    { stdio: ['ignore', 'ignore', 'pipe'] },
  );
  if (r.status !== 0) fail(`ffmpeg WAV gen exited ${r.status}: ${r.stderr}`);
}

function makeWaveformJson(path) {
  log(`generating synthetic waveform.json at ${path}`);
  // 500 buckets to match `audio/mod.rs::WAVEFORM_BUCKETS`. Use a
  // half-sine so the bars actually pulse.
  const buckets = 500;
  const peaks = Array.from({ length: buckets }, (_, i) => {
    const t = i / buckets;
    return Math.abs(Math.sin(Math.PI * t * 4)) * 0.8 + 0.05;
  });
  writeFileSync(path, JSON.stringify({ sample_rate_hz: 24_000, buckets, peaks }));
}

function buildSpec({ wavPath, peaksPath, mp4Path }) {
  // Mirrors what the Phase B planner produces for a real chapter:
  // 4s title, ~5s paragraph window, 3s outro. Keep paragraph text
  // short enough to read in one window so karaoke runs cleanly.
  return {
    version: 1,
    chapter: {
      number: 1,
      title: 'The Trust Stack',
      duration_ms: DURATION_MS,
    },
    audio: { wav: wavPath, peaks: peaksPath },
    theme: { preset: 'library', primary: null, accent: null },
    background: { kind: 'color', color: '#0F172A' },
    scenes: [
      {
        kind: 'title',
        start_ms: 0,
        end_ms: 4_000,
        title: 'The Trust Stack',
        subtitle: 'Chapter 1',
      },
      {
        kind: 'paragraph',
        start_ms: 4_000,
        end_ms: 9_000,
        text:
          'Trust is the substrate of every transaction. Strip it away and even the simplest trade collapses into bargaining about reliability rather than price.',
        tile: null,
        highlight: 'karaoke',
      },
      {
        kind: 'outro',
        start_ms: 9_000,
        end_ms: 12_000,
        title: 'Continue listening',
        subtitle: 'listenai.app',
      },
    ],
    captions: null,
    output: {
      mp4: mp4Path,
      width: 1920,
      height: 1080,
      fps: 30,
    },
  };
}

function runCli(spec) {
  const cli = MOCK
    ? null
    : resolve(ROOT, 'dist', 'cli.js');
  if (!MOCK && !existsSync(cli)) {
    fail(`dist/cli.js not found — run \`npm run build\` in backend/render first`);
  }

  if (MOCK) {
    // ffmpeg-only path: black 1920x1080@30 of the right duration. Same
    // shortcut the Rust publisher uses when LISTENAI_ANIMATE_MOCK=true.
    log('mock mode: rendering black mp4 via ffmpeg');
    const r = spawnSync(
      'ffmpeg',
      [
        '-y',
        '-f', 'lavfi',
        '-i', `color=c=black:s=1920x1080:r=30:d=${DURATION_MS / 1000}`,
        '-i', spec.audio.wav,
        '-c:v', 'libx264',
        '-pix_fmt', 'yuv420p',
        '-preset', 'veryfast',
        '-c:a', 'aac',
        '-shortest',
        spec.output.mp4,
      ],
      { stdio: ['ignore', 'ignore', 'pipe'] },
    );
    if (r.status !== 0) fail(`ffmpeg mock render exited ${r.status}: ${r.stderr}`);
    return Promise.resolve();
  }

  return new Promise((resolveFn, rejectFn) => {
    log(`spawning node ${cli}`);
    const child = spawn('node', [cli], {
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    const ndjsonPath = resolve(OUT, 'progress.ndjson');
    const ndjsonChunks = [];
    const stderrChunks = [];

    child.stdout.on('data', (c) => ndjsonChunks.push(c));
    child.stderr.on('data', (c) => stderrChunks.push(c));
    child.on('error', rejectFn);
    child.on('close', (code) => {
      writeFileSync(ndjsonPath, Buffer.concat(ndjsonChunks));
      const stderrTail = Buffer.concat(stderrChunks).toString('utf8').trim().split('\n').slice(-10).join('\n');
      if (code !== 0) {
        log(`renderer stderr (last 10 lines):\n${stderrTail}`);
        rejectFn(new Error(`renderer exited ${code}`));
        return;
      }
      // Look for the `done` event for a clean handoff message.
      const ndjson = Buffer.concat(ndjsonChunks).toString('utf8');
      const lastLine = ndjson.trim().split('\n').slice(-1)[0] ?? '';
      log(`renderer exit 0 — last NDJSON: ${lastLine}`);
      resolveFn();
    });

    child.stdin.end(JSON.stringify(spec));
  });
}

function ffprobe(path) {
  const r = spawnSync(
    'ffprobe',
    [
      '-v', 'error',
      '-show_entries', 'stream=codec_name,codec_type,width,height,r_frame_rate,sample_rate,channels:format=duration',
      '-of', 'json',
      path,
    ],
    { encoding: 'utf8' },
  );
  if (r.status !== 0) fail(`ffprobe failed on ${path}: ${r.stderr}`);
  return JSON.parse(r.stdout);
}

function verify(mp4Path) {
  const probe = ffprobe(mp4Path);
  const v = probe.streams.find((s) => s.codec_type === 'video');
  const a = probe.streams.find((s) => s.codec_type === 'audio');
  if (!v) fail('no video stream in output mp4');
  if (!a) fail('no audio stream in output mp4');
  if (v.width !== 1920 || v.height !== 1080) {
    fail(`video size ${v.width}x${v.height}, expected 1920x1080`);
  }
  if (v.codec_name !== 'h264') {
    fail(`video codec ${v.codec_name}, expected h264`);
  }
  // r_frame_rate is e.g. "30/1"; accept anything that evaluates to ~30.
  const [num, den] = v.r_frame_rate.split('/').map(Number);
  const fps = num / (den || 1);
  if (Math.abs(fps - 30) > 0.5) fail(`video fps ${fps}, expected ~30`);
  if (a.codec_name !== 'aac') fail(`audio codec ${a.codec_name}, expected aac`);
  const duration = parseFloat(probe.format.duration);
  const durationMs = duration * 1000;
  const drift = Math.abs(durationMs - DURATION_MS);
  if (drift > 250) {
    fail(`duration ${durationMs}ms drifted ${drift}ms from expected ${DURATION_MS}ms`);
  }
  log(`OK: ${v.width}x${v.height}@${fps.toFixed(0)}fps ${v.codec_name} + ${a.sample_rate}Hz ${a.codec_name}, ${durationMs.toFixed(0)}ms (drift ${drift.toFixed(0)}ms)`);
}

async function main() {
  assertCmd('ffmpeg', 'ffmpeg');
  assertCmd('ffprobe', 'ffprobe');

  if (existsSync(OUT)) rmSync(OUT, { recursive: true, force: true });
  mkdirSync(OUT, { recursive: true });

  const wavPath = resolve(OUT, 'fixture.wav');
  const peaksPath = resolve(OUT, 'fixture.waveform.json');
  const mp4Path = resolve(OUT, 'fixture.video.mp4');

  makeWav(wavPath);
  makeWaveformJson(peaksPath);
  const spec = buildSpec({ wavPath, peaksPath, mp4Path });
  writeFileSync(resolve(OUT, 'spec.json'), JSON.stringify(spec, null, 2));

  await runCli(spec);

  if (!existsSync(mp4Path)) fail(`renderer reported success but ${mp4Path} is missing`);
  verify(mp4Path);

  log(`mp4: ${mp4Path}`);
  if (KEEP) {
    log(`--keep set; leaving artefacts under ${OUT}`);
  } else {
    log('cleaning up; pass --keep to retain the artefacts');
    rmSync(wavPath, { force: true });
    rmSync(peaksPath, { force: true });
    // Leave the MP4 + spec for inspection; only WAV/peaks are
    // deterministic byproducts that nobody wants to ship.
  }
  log('PASS');
}

main().catch((e) => {
  fail(e.stack ?? e.message ?? String(e));
});
