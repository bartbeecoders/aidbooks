// Full-bleed chapter background. Phase A used a static cover image at
// half opacity; Phase C adds:
//   * Ken Burns slow zoom-and-pan over the whole video duration
//     (1.00 → 1.10 scale, ~5% pan along x).
//   * Theme overlay tint baked on top so card text stays legible
//     regardless of the cover's local contrast.
//   * A solid-colour fallback when there's no cover image (matches the
//     theme's `background` colour).
//
// Returns the Layout node + a long-running animation generator the
// orchestrator launches once at scene start.

import { Img, Layout, Rect } from '@revideo/2d';
import type { Node } from '@revideo/2d';
import { createRef, tween, waitFor } from '@revideo/core';
import type { ThreadGenerator } from '@revideo/core';

import type { Theme } from '../themes';

export interface BackgroundInput {
  kind: 'image' | 'color';
  src?: string | null;
  color?: string | null;
}

export interface Background {
  node: Node;
  /** Run the Ken Burns pan over the whole video duration. The
   * orchestrator launches this in parallel with the card timeline. */
  animate: (totalSec: number) => ThreadGenerator;
}

export function makeBackground(input: BackgroundInput, theme: Theme): Background {
  // Solid-colour case is just one Rect; nothing to animate.
  if (input.kind !== 'image' || !input.src) {
    const fill = input.color ?? theme.background;
    const node = (
      <Rect width={1920} height={1080} fill={fill} />
    ) as unknown as Node;
    return {
      node,
      // Solid-fill case has no animation, but the generator must still
      // run for the full chapter duration — Revideo's `recalculate()`
      // pass measures the scene's frame count by walking each
      // generator to completion, and an instantly-returning generator
      // here would let the other parallel branches in `all()` cut the
      // measured duration short. `waitFor` keeps the strip on screen
      // for the whole video without animating it.
      animate: function* (totalSec: number): ThreadGenerator {
        yield* waitFor(totalSec);
      },
    };
  }

  const img = createRef<Img>();
  const node = (
    <Layout width={1920} height={1080} clip>
      <Rect width={1920} height={1080} fill={theme.background} />
      <Img
        ref={img}
        src={input.src}
        width={1920}
        height={1080}
        scale={1.0}
        x={0}
      />
      <Rect width={1920} height={1080} fill={theme.overlay} />
    </Layout>
  ) as unknown as Node;

  function* animate(totalSec: number): ThreadGenerator {
    if (totalSec <= 0) {
      return;
    }
    // Slow linear zoom + pan. Linear (not eased) because the eye reads
    // any easing on a long zoom as "the camera lurched".
    yield* tween(totalSec, (t) => {
      const scale = 1.0 + 0.10 * t;
      const x = -40 * t; // small horizontal drift
      img().scale(scale);
      img().x(x);
    });
  }

  return { node, animate };
}
