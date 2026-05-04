// Scene orchestrator. Consumes the full `SceneSpec` from
// `useScene().variables.get('spec', ...)`, builds the static node tree
// (Background, all the cards, WaveformPulse), then walks the
// SceneSpec.scenes timeline yielding each card's `show()` at the
// right moment.
//
// Card lifecycle:
//   * Background and WaveformPulse are continuous — their `animate()`
//     generators run in parallel for the full chapter duration.
//   * Title / Paragraph / Outro are sequenced — at each scene's
//     `start_ms` the orchestrator switches the active card to opacity
//     1 (its own `show()` handles the fade), waits for the scene's
//     duration, then moves on.
//
// We also play the chapter's WAV via a single `<Audio play/>` so
// the rendered MP4 has the narration baked in. Revideo muxes audio
// alongside frames at the encoding stage.

import { Audio, makeScene2D } from '@revideo/2d';
import type { View2D } from '@revideo/2d';
import { all, createRef, useScene, waitFor } from '@revideo/core';
import type { ThreadGenerator } from '@revideo/core';

import { makeBackground } from './components/Background';
import { makeOutro } from './components/Outro';
import { makeParagraphScene } from './components/ParagraphScene';
import { makeTitleCard } from './components/TitleCard';
import { makeWaveformPulse } from './components/WaveformPulse';
import type { Card } from './components/types';
import { resolveTheme } from './themes';
import type { SceneSpec } from './spec';

// Default to a synthetic minimal spec so a misrouted invocation still
// renders something rather than throwing on an undefined signal. In
// practice cli.ts always passes the spec.
const FALLBACK_SPEC: SceneSpec = {
  version: 1,
  chapter: { number: 0, title: '', duration_ms: 1000 },
  audio: { wav: '', peaks: null },
  theme: { preset: 'library', primary: null, accent: null },
  background: { kind: 'color', color: '#0F172A' },
  scenes: [],
  captions: null,
  output: { mp4: '', width: 1920, height: 1080, fps: 30 },
};

export default makeScene2D('chapter', function* (view: View2D) {
  // Revideo's variables API is signal-based: `get(name, default)`
  // returns a `() => T` getter. We call it once at scene init since
  // the spec never changes during a single render. Until Phase C we
  // were reading `variables.spec` directly — that's always undefined,
  // which is why earlier renders silently fell back to the 1s
  // FALLBACK_SPEC.
  const specSignal = useScene().variables.get<SceneSpec>('spec', FALLBACK_SPEC);
  const spec: SceneSpec = specSignal();

  const theme = resolveTheme(
    spec.theme.preset,
    spec.theme.primary,
    spec.theme.accent,
  );
  const totalSec = spec.chapter.duration_ms / 1000;

  // 1. Build the always-on layers.
  const background = makeBackground(
    spec.background.kind === 'image'
      ? { kind: 'image', src: spec.background.src }
      : { kind: 'color', color: spec.background.color },
    theme,
  );
  const waveform = makeWaveformPulse(spec.audio.peaks ?? null, theme);

  // 2. Build one card per scene. We materialise the cards up front so
  // the JSX tree is stable; the timeline-walk below just toggles which
  // is on screen.
  const cards: Card[] = spec.scenes.map((s) => {
    if (s.kind === 'title') {
      return makeTitleCard(
        {
          number: spec.chapter.number,
          title: s.title,
          subtitle: s.subtitle ?? null,
        },
        theme,
      );
    }
    if (s.kind === 'paragraph') {
      return makeParagraphScene(
        {
          text: s.text,
          tile: s.tile ?? null,
          highlight: s.highlight ?? 'karaoke',
        },
        theme,
      );
    }
    return makeOutro(
      { title: s.title, subtitle: s.subtitle ?? null },
      theme,
    );
  });

  // 3. Add everything to the view in z-order: bg → cards → waveform → audio.
  const audioRef = createRef<Audio>();
  view.add(background.node);
  for (const c of cards) {
    view.add(c.node);
  }
  view.add(waveform.node);
  if (spec.audio.wav) {
    view.add(<Audio ref={audioRef} src={spec.audio.wav} play={true} />);
  }

  // 4. Walk the timeline. The cards' opacity starts at 0; each card's
  // `show()` runs the fade-in, hold, fade-out internally. Background
  // and waveform run for the full duration in parallel via `all()`.
  function* timeline(): ThreadGenerator {
    let cursor = 0;
    for (let i = 0; i < spec.scenes.length; i++) {
      const s = spec.scenes[i];
      if (s.start_ms > cursor) {
        yield* waitFor((s.start_ms - cursor) / 1000);
      }
      const dur = (s.end_ms - s.start_ms) / 1000;
      yield* cards[i].show(dur);
      cursor = s.end_ms;
    }
    // Tail: if the last scene ends before the audio, hold on the
    // background until the WAV finishes so the MP4 covers the whole
    // narration.
    if (cursor < spec.chapter.duration_ms) {
      yield* waitFor((spec.chapter.duration_ms - cursor) / 1000);
    }
  }

  yield* all(
    background.animate(totalSec),
    waveform.animate(totalSec),
    timeline(),
  );
});
