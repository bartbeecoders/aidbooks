// Long-lived renderer sidecar.
//
// Boots once, processes many SceneSpec JSON requests over its lifetime
// (one per line of stdin), keeps the heavy module imports + Vite/Chromium
// context warm between renders, and exits cleanly when stdin closes.
//
// Protocol (NDJSON, both directions):
//
//   stdin  : one SceneSpec JSON object per line. Closing stdin = clean
//            shutdown (the server processes any in-flight render and
//            exits).
//   stdout : same NDJSON event stream as `cli.ts`, plus:
//              * `{type:"ready"}` once at boot, and again after every
//                render (success or failure) — the Rust pool waits on
//                this before sending the next spec.
//              * `{type:"bye"}`   immediately before exit on EOF.
//
// Exit codes:
//   0 — clean shutdown (stdin closed).
//   1 — fatal error during boot (the error event has been emitted).
//
// Per-render failures stay non-fatal: the server emits `error` then
// `ready` and waits for the next spec.

import { stdin } from 'node:process';
import readline from 'node:readline';

import { parseSpec } from './spec.js';
import { emit } from './events.js';
import { renderOne } from './render.js';

async function main(): Promise<number> {
  const rl = readline.createInterface({
    input: stdin,
    crlfDelay: Infinity,
  });

  // Tell the Rust pool we're listening before we touch any input.
  emit({ type: 'ready' });

  for await (const rawLine of rl) {
    const line = rawLine.trim();
    if (!line) continue;

    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch (e) {
      emit({
        type: 'error',
        message: `invalid JSON on stdin: ${(e as Error).message}`,
      });
      emit({ type: 'ready' });
      continue;
    }

    let spec;
    try {
      spec = parseSpec(parsed);
    } catch (e) {
      emit({
        type: 'error',
        message: `invalid SceneSpec: ${(e as Error).message}`,
      });
      emit({ type: 'ready' });
      continue;
    }

    // renderOne emits started/frame/encoding/done or error itself.
    // We always follow the render with a `ready` so the pool can
    // dispatch the next spec regardless of outcome.
    try {
      await renderOne(spec);
    } catch (e) {
      // renderOne swallows its own errors and returns 1; if anything
      // escapes anyway, log it and keep the sidecar alive.
      emit({
        type: 'error',
        message: `render threw: ${(e as Error).message}`,
      });
    }
    emit({ type: 'ready' });
  }

  emit({ type: 'bye' });
  return 0;
}

main()
  .then((code) => process.exit(code))
  .catch((e) => {
    emit({ type: 'error', message: `sidecar fatal: ${(e as Error).message}` });
    process.exit(1);
  });
