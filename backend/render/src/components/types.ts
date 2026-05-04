// Shared types for the scene component library. The orchestrator
// (`scene.tsx`) builds a node tree once, then walks the timeline
// yielding each card's `show(durationSec)` at the right time. The
// generator handles fade-in → hold → fade-out internally; the
// orchestrator just hands it the window length and waits for it to
// return.
//
// This keeps the components focused (one card = one file) and the
// orchestrator dumb (just a timeline cursor + some `yield*`s).

import type { Node } from '@revideo/2d';
import type { ThreadGenerator } from '@revideo/core';

export interface Card {
  /** Pre-built node added to the view at scene init. The orchestrator
   * doesn't read this — it only needs it long enough to attach. */
  node: Node;
  /** Run the card's intro/hold/outro for `durationSec` seconds. Cards
   * own their own fade timing; callers just `yield*` and move on. */
  show: (durationSec: number) => ThreadGenerator;
}
