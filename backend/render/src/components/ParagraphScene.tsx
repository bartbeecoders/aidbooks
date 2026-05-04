// Paragraph scene: left-side paragraph illustration tile (or the
// chapter background showing through if no tile), right-side karaoke
// text reveal at a constant per-character cadence over the scene
// window.
//
// Constant-cadence karaoke means: char N appears at
// `start + (N / total_chars) * scene_duration`. No Whisper word
// timing in v1 — that's a Phase F upgrade. The cadence still feels
// natural for prose because TTS narration is itself ~uniform in
// chars/second.
//
// Show() lifecycle:
//   * 0.0 .. 0.5 s   fade in, tile crossfades up
//   * 0.5 .. dur-0.5 karaoke reveal driven by `revealedChars` signal
//   * dur-0.5 .. dur fade out

import { Img, Layout, Rect, Txt } from '@revideo/2d';
import type { Node } from '@revideo/2d';
import { createRef, createSignal, tween } from '@revideo/core';
import type { ThreadGenerator } from '@revideo/core';

import type { Theme } from '../themes';
import type { Card } from './types';

export interface ParagraphSceneInput {
  text: string;
  /** Optional tile URL (already converted from absolute path to
   * `file://` URL by the caller). */
  tile?: string | null;
  /** Highlight strategy. v1 supports `karaoke` (per-char reveal) and
   * `none` (text shown statically). Anything else falls back to
   * `none`. */
  highlight: string;
}

export function makeParagraphScene(input: ParagraphSceneInput, theme: Theme): Card {
  const root = createRef<Layout>();
  const tileImg = createRef<Img>();
  const tileFrame = createRef<Rect>();

  // Karaoke state — number of revealed characters. Drives the visible
  // and revealed text spans below.
  const revealed = createSignal(0);
  const totalChars = input.text.length;

  // Two-text approach: a low-opacity full text underneath, an
  // accent-coloured "already-read" span on top. Cleaner than splitting
  // the string at draw time because the dim layer keeps wrapping
  // stable while the on-top layer grows.
  const dimText = (() => input.text);
  const litText = () => input.text.slice(0, Math.min(totalChars, Math.round(revealed())));

  const hasTile = !!input.tile;

  const node = (
    <Layout
      ref={root}
      width={1920}
      height={1080}
      direction={'row'}
      alignItems={'center'}
      justifyContent={'center'}
      gap={hasTile ? 64 : 0}
      padding={[80, 120]}
      opacity={0}
      layout
    >
      {hasTile ? (
        <Layout direction={'column'} alignItems={'center'} layout>
          <Rect
            ref={tileFrame}
            width={720}
            height={720}
            fill={theme.overlay}
            radius={24}
            clip
          >
            <Img
              ref={tileImg}
              src={input.tile!}
              width={720}
              height={720}
            />
          </Rect>
        </Layout>
      ) : null}
      <Layout
        direction={'column'}
        alignItems={'start'}
        justifyContent={'center'}
        maxWidth={hasTile ? 980 : 1600}
        layout
      >
        {/* Two stacked Txts: dim baseline + lit overlay. They're
            wrapped together via a parent Layout in column direction so
            the overlay sits exactly on top of the baseline at the same
            wrap points. We set the baseline via opacity rather than
            colour to stay theme-agnostic. */}
        <Layout direction={'column'} layout>
          <Txt
            text={dimText}
            fontFamily={theme.fontFamily}
            fontWeight={400}
            fontSize={48}
            lineHeight={68}
            fill={theme.secondary}
            opacity={0.45}
            textWrap={true}
            maxWidth={hasTile ? 980 : 1600}
          />
          <Txt
            text={litText}
            fontFamily={theme.fontFamily}
            fontWeight={500}
            fontSize={48}
            lineHeight={68}
            fill={theme.primary}
            textWrap={true}
            maxWidth={hasTile ? 980 : 1600}
            // Pull up so the overlay sits on the dim baseline.
            offsetY={1}
            marginTop={-68 * Math.max(1, Math.ceil(input.text.length / 60))}
          />
        </Layout>
      </Layout>
    </Layout>
  ) as unknown as Node;

  function* show(durationSec: number): ThreadGenerator {
    const fade = Math.min(0.5, durationSec / 4);
    // Fade in + tile zoom-in.
    yield* tween(fade, (t) => {
      root().opacity(t);
      if (hasTile) {
        tileFrame().scale(0.94 + 0.06 * t);
      }
    });
    // Karaoke reveal across the hold. Per-char cadence.
    const holdSec = Math.max(0, durationSec - 2 * fade);
    if (input.highlight === 'karaoke' && totalChars > 0 && holdSec > 0) {
      yield* tween(holdSec, (t) => {
        revealed(totalChars * t);
      });
    } else {
      // No karaoke — show full text statically for the whole hold.
      revealed(totalChars);
      yield* tween(holdSec, (_t) => {});
    }
    yield* tween(fade, (t) => {
      root().opacity(1 - t);
    });
  }

  return { node, show };
}
