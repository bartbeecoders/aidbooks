// Waveform-reactive accent strip along the bottom of the frame.
// Reads the chapter's pre-computed peaks file (`ch-N.waveform.json`)
// — the same one the player UI uses — and pulses a thin horizontal
// bar in the theme's accent colour.
//
// Why we use the precomputed peaks rather than wiring Revideo's audio
// API: the peaks file is what we ship to the player anyway, so the
// reactive cue here matches what the listener will see in the web
// app. It's also far cheaper than running an FFT at render time —
// the file is 500 floats and we sample it linearly.
//
// Drawing strategy: the strip sits at y=480 (bottom 60 px of the
// 1080 frame), full-width. We render it as a sequence of evenly
// spaced vertical Rects whose heights track the local peak. The
// alternative — one wide Rect with an animated height — looks
// muddier on the eye than the bar-graph rendering.

import { Layout, Rect } from '@revideo/2d';
import type { Node } from '@revideo/2d';
import { createRef, createSignal, tween } from '@revideo/core';
import type { ThreadGenerator } from '@revideo/core';

import type { Theme } from '../themes';

const BAR_COUNT = 96;
const BAR_GAP_PX = 4;
const STRIP_HEIGHT_PX = 56;
const STRIP_BOTTOM_OFFSET_PX = 36; // pixels above the frame's bottom edge

interface PeaksFile {
  peaks: number[];
  buckets?: number;
}

export interface WaveformPulse {
  node: Node;
  /** Run the pulse for the entire video duration. The orchestrator
   * launches this in parallel with the card timeline. */
  animate: (totalSec: number) => ThreadGenerator;
}

/**
 * Construct the bar strip. If `peaksUrl` is null or fetch fails the
 * component renders an empty (zero-height) row — never an exception,
 * so a missing peaks file just dims the visual instead of failing the
 * whole render.
 */
export function makeWaveformPulse(
  peaksUrl: string | null,
  theme: Theme,
): WaveformPulse {
  const bars = Array.from({ length: BAR_COUNT }, () =>
    createSignal(0.05),
  );
  const root = createRef<Layout>();

  const barWidthPx =
    Math.floor((1920 - BAR_GAP_PX * (BAR_COUNT - 1)) / BAR_COUNT);

  const node = (
    <Layout
      ref={root}
      x={0}
      y={1080 / 2 - STRIP_HEIGHT_PX / 2 - STRIP_BOTTOM_OFFSET_PX}
      width={1920}
      height={STRIP_HEIGHT_PX}
      direction={'row'}
      alignItems={'center'}
      gap={BAR_GAP_PX}
      opacity={0.85}
      layout
    >
      {bars.map((amp, i) => (
        <Rect
          key={`wf-${i}`}
          width={barWidthPx}
          height={() => Math.max(2, amp() * STRIP_HEIGHT_PX)}
          fill={theme.accent}
          radius={2}
        />
      ))}
    </Layout>
  ) as unknown as Node;

  // Lazy-load peaks. Errors are swallowed (logged to stderr by the
  // orchestrator) so a missing file doesn't fail the render.
  let peaksPromise: Promise<number[]> | null = null;
  function loadPeaks(): Promise<number[]> {
    if (peaksPromise) return peaksPromise;
    if (!peaksUrl) {
      peaksPromise = Promise.resolve([]);
      return peaksPromise;
    }
    peaksPromise = fetch(peaksUrl)
      .then((r) => r.json())
      .then((data: PeaksFile) => data.peaks ?? [])
      .catch((e) => {
        console.error(`waveform fetch failed: ${(e as Error).message}`);
        return [];
      });
    return peaksPromise;
  }

  function sample(peaks: number[], normPos: number): number {
    if (peaks.length === 0) return 0;
    const idx = Math.min(
      peaks.length - 1,
      Math.floor(normPos * peaks.length),
    );
    return peaks[idx];
  }

  function* animate(totalSec: number): ThreadGenerator {
    if (totalSec <= 0) return;
    const peaks = yield loadPeaks();
    const peaksArr = (peaks as unknown as number[]) ?? [];

    // Drive each bar from a windowed slice of the peaks array centred
    // on the current playhead. Squaring softens noise — peaks JSON
    // values are already in [0,1] but visually quiet sections still
    // show non-zero amplitude that becomes distracting on a tall bar.
    yield* tween(totalSec, (t) => {
      const playhead = t; // 0..1 over total duration
      for (let i = 0; i < BAR_COUNT; i++) {
        const offset = (i - BAR_COUNT / 2) * 0.0008; // fan-out across ~75ms either side
        const local = Math.max(0, Math.min(1, playhead + offset));
        const raw = sample(peaksArr, local);
        // Square + small floor so the strip never goes fully flat.
        const amp = Math.max(0.04, raw * raw);
        bars[i](amp);
      }
    });
  }

  return { node, animate };
}
