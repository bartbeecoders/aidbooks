// Closing card. Mirrors TitleCard structurally — same fonts and
// underline accent — so the chapter's visual bookends feel
// consistent. Phase F may add a per-book CTA QR code; v1 keeps it
// to text only.

import { Layout, Rect, Txt } from '@revideo/2d';
import type { Node } from '@revideo/2d';
import { createRef, tween } from '@revideo/core';
import type { ThreadGenerator } from '@revideo/core';

import type { Theme } from '../themes';
import type { Card } from './types';

export interface OutroInput {
  title: string;
  subtitle?: string | null;
}

export function makeOutro(input: OutroInput, theme: Theme): Card {
  const root = createRef<Layout>();
  const accent = createRef<Rect>();

  const node = (
    <Layout
      ref={root}
      width={1920}
      height={1080}
      direction={'column'}
      alignItems={'center'}
      justifyContent={'center'}
      gap={28}
      opacity={0}
      layout
    >
      <Rect ref={accent} width={0} height={4} fill={theme.accent} radius={2} />
      <Txt
        text={input.title}
        fontFamily={theme.fontFamily}
        fontWeight={theme.headingWeight}
        fontSize={72}
        fill={theme.primary}
        textAlign={'center'}
        textWrap={true}
        maxWidth={1500}
      />
      {input.subtitle ? (
        <Txt
          text={input.subtitle}
          fontFamily={theme.fontFamily}
          fontWeight={400}
          fontSize={32}
          fill={theme.secondary}
          maxWidth={1500}
          textAlign={'center'}
        />
      ) : null}
    </Layout>
  ) as unknown as Node;

  function* show(durationSec: number): ThreadGenerator {
    const fade = Math.min(0.5, durationSec / 4);
    yield* tween(fade, (t) => {
      root().opacity(t);
      accent().width(220 * t);
    });
    yield* tween(durationSec - 2 * fade, (_t) => {});
    yield* tween(fade, (t) => {
      root().opacity(1 - t);
    });
  }

  return { node, show };
}
