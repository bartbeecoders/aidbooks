// Shared render core. Consumed by:
//   * `cli.ts` — one-shot mode (one spec on stdin, exit after).
//   * `server.ts` — long-lived sidecar (many specs over the lifetime
//     of the process, with `ready` events between renders).
//
// `renderOne` returns 0 on success, 1 on a render failure that has
// already been emitted as a `{type:"error"}` event. Both wrappers
// translate the return code into their respective process semantics.

import { mkdirSync } from 'node:fs';
import { basename, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Hoisted to module scope so the long-lived sidecar pays the
// `@revideo/renderer` import cost exactly once across all renders. The
// one-shot CLI also benefits — the import lands during module load
// before `parseSpec` runs, but for a one-shot that's a wash either way.
import { renderVideo } from '@revideo/renderer';

import { emit } from './events.js';
import type { SceneSpec } from './spec.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

function pathToUrl(p: string): string {
  // Revideo's <Audio>/<Img> components fetch their `src` via the Vite
  // dev server (Chromium can't fetch `file://` URLs from a page served
  // over HTTP). Vite serves arbitrary host files via the `/@fs/<abs>`
  // route when `fs.allow` permits it; we open `fs.allow` to root in
  // `viteConfig` below. Empty strings stay empty — the scene checks
  // `if (spec.audio.wav)` before adding the <Audio> component.
  if (!p) return '';
  const abs = resolve(p);
  return `/@fs${abs}`;
}

/**
 * Render one chapter MP4 from a parsed `SceneSpec`. Emits the same
 * `started` / `frame` / `encoding` / `done` / `error` NDJSON sequence
 * that the original one-shot CLI emitted.
 *
 * Resolves to `0` on success, `1` on a render-time failure (the
 * `error` event has already been emitted). Does not emit `ready` —
 * that's the wrapper's job.
 */
export async function renderOne(spec: SceneSpec): Promise<0 | 1> {
  emit({ type: 'started' });

  // Rewrite all on-disk paths inside the spec into `/@fs<abs>` URLs
  // and pass the whole structured spec through as a single variable.
  // The scene orchestrator (`scene.tsx`) reads it back via
  // `useScene().variables.get('spec')` and dispatches per-Scene.
  const urlSpec = {
    ...spec,
    audio: {
      ...spec.audio,
      wav: pathToUrl(spec.audio.wav),
      peaks: spec.audio.peaks ? pathToUrl(spec.audio.peaks) : null,
    },
    background:
      spec.background.kind === 'image'
        ? { ...spec.background, src: pathToUrl(spec.background.src) }
        : spec.background,
    scenes: spec.scenes.map((s) =>
      s.kind === 'paragraph' && s.tile
        ? { ...s, tile: pathToUrl(s.tile) }
        : s,
    ),
  };

  const variables = {
    spec: urlSpec,
  };

  // Revideo compiles the project via Vite at render time — we hand it
  // the source `.tsx`. `dist/render.js` lives alongside the renderer's
  // `dist/`, so `../src/project.tsx` resolves back to the canonical
  // source regardless of the wrapper's CWD.
  const projectFile = resolve(__dirname, '..', 'src', 'project.tsx');

  const outAbs = resolve(spec.output.mp4);
  const outDir = dirname(outAbs);
  const outFile = basename(outAbs);
  if (!outFile.endsWith('.mp4')) {
    emit({
      type: 'error',
      message: `output.mp4 must end in .mp4 (got "${outFile}")`,
    });
    return 1;
  }
  try {
    mkdirSync(outDir, { recursive: true });
  } catch (e) {
    emit({
      type: 'error',
      message: `create output dir ${outDir}: ${(e as Error).message}`,
    });
    return 1;
  }

  // Revideo's `progressCallback` reports a single 0..1 fraction per
  // worker. Translate it to our coarser frame/encoding events so the
  // Rust side's progress hub looks the same as for any other job.
  const totalFrames = Math.max(
    1,
    Math.round((spec.chapter.duration_ms / 1000) * spec.output.fps),
  );
  let lastFrame = 0;

  try {
    const result = await renderVideo({
      projectFile,
      settings: {
        outDir,
        outFile: outFile as `${string}.mp4`,
        workers: 1,
        logProgress: false,
        viteConfig: {
          server: { fs: { allow: ['/'], strict: false } },
        },
        projectSettings: {
          exporter: {
            name: '@revideo/core/ffmpeg',
            options: { format: 'mp4' },
          },
        },
        progressCallback: (_worker, progress) => {
          const frame = Math.max(
            lastFrame,
            Math.min(totalFrames, Math.round(progress * totalFrames)),
          );
          if (frame > lastFrame) {
            emit({ type: 'frame', frame, total: totalFrames });
            lastFrame = frame;
          }
        },
      },
      variables,
    });

    const mp4 = typeof result === 'string' ? result : outAbs;
    emit({ type: 'encoding', pct: 1.0 });
    emit({ type: 'done', mp4, duration_ms: spec.chapter.duration_ms });
    return 0;
  } catch (e) {
    emit({ type: 'error', message: `render failed: ${(e as Error).message}` });
    return 1;
  }
}
