// Chapter title card: large "Chapter N" line above the chapter title,
// thin animated accent underline. Used at the very start of each
// chapter video. Sits on top of the Background and below the
// WaveformPulse.
//
// Show() lifecycle:
//   * 0.0 .. 0.5 s     fade in, accent line draws left → right
//   * 0.5 .. dur-0.5 s hold
//   * dur-0.5 .. dur s fade out

import { Layout, Rect, Txt } from '@revideo/2d';
import type { Node } from '@revideo/2d';
import { createRef, tween } from '@revideo/core';
import type { ThreadGenerator } from '@revideo/core';

import type { Theme } from '../themes';
import type { Card } from './types';

export interface TitleCardInput {
  number: number;
  title: string;
  subtitle?: string | null;
}

export function makeTitleCard(input: TitleCardInput, theme: Theme): Card {
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
      <Txt
        text={`Chapter ${input.number}`}
        fontFamily={theme.fontFamily}
        fontWeight={400}
        fontSize={48}
        fill={theme.secondary}
      />
      <Txt
        text={input.title}
        fontFamily={theme.fontFamily}
        fontWeight={theme.headingWeight}
        fontSize={108}
        fill={theme.primary}
        textAlign={'center'}
        textWrap={true}
        maxWidth={1500}
      />
      <Rect ref={accent} width={0} height={6} fill={theme.accent} radius={3} />
      {input.subtitle ? (
        <Txt
          text={input.subtitle}
          fontFamily={theme.fontFamily}
          fontWeight={400}
          fontSize={36}
          fill={theme.secondary}
          maxWidth={1500}
          textAlign={'center'}
        />
      ) : null}
    </Layout>
  ) as unknown as Node;

  function* show(durationSec: number): ThreadGenerator {
    const fade = Math.min(0.5, durationSec / 4);
    // Fade in + accent draw together.
    yield* tween(fade, (t) => {
      root().opacity(t);
      accent().width(280 * t);
    });
    yield* tween(durationSec - 2 * fade, (_t) => {
      // Hold; nothing animates. Could parallax the accent here later.
    });
    yield* tween(fade, (t) => {
      root().opacity(1 - t);
    });
  }

  return { node, show };
}
