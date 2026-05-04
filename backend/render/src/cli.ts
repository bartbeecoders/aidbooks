// One-shot renderer entry point.
//
// Reads a single SceneSpec JSON from stdin, renders it, exits.
// Emits NDJSON progress on stdout (`started` / `frame` / `encoding` /
// `done` / `error`); never emits `ready` (that's the long-lived
// `server.ts` sidecar).
//
// Used by the standalone demo script
// (`scripts/animate-single-chapter.sh`) and the test harness. Production
// renders go through `server.ts` / the Rust `RendererPool`.

import { parseSpec } from './spec.js';
import { emit } from './events.js';
import { renderOne } from './render.js';

async function readStdin(): Promise<string> {
  return new Promise((resolveFn, rejectFn) => {
    const chunks: Buffer[] = [];
    process.stdin.on('data', (c: Buffer | string) => {
      chunks.push(typeof c === 'string' ? Buffer.from(c) : c);
    });
    process.stdin.on('end', () => resolveFn(Buffer.concat(chunks).toString('utf8')));
    process.stdin.on('error', rejectFn);
  });
}

async function main(): Promise<number> {
  const raw = await readStdin();
  if (!raw.trim()) {
    emit({ type: 'error', message: 'empty stdin — expected SceneSpec JSON' });
    return 2;
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (e) {
    emit({ type: 'error', message: `invalid JSON: ${(e as Error).message}` });
    return 2;
  }

  let spec;
  try {
    spec = parseSpec(parsed);
  } catch (e) {
    emit({ type: 'error', message: `invalid SceneSpec: ${(e as Error).message}` });
    return 2;
  }

  return renderOne(spec);
}

main()
  .then((code) => process.exit(code))
  .catch((e) => {
    emit({ type: 'error', message: `unhandled: ${(e as Error).message}` });
    process.exit(1);
  });
