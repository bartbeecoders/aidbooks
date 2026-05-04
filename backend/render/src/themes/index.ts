// Theme presets for the animation renderer. Three for v1 — adding a
// fourth means a new entry in `PRESETS`; consumers go through
// `resolveTheme` so they can't reach for an undefined preset name.
//
// A theme is just colours + a font stack. Per-component tuning
// (font sizes, spacing) lives in the component, not here, so the
// theme stays trivial to swap mid-render.

export interface Theme {
  preset: string;
  /** Page background tint behind the cover image. */
  background: string;
  /** Foreground text colour for headlines (TitleCard, Outro). */
  primary: string;
  /** Secondary text colour for subtitles + body. */
  secondary: string;
  /** Accent colour — TitleCard underline, WaveformPulse bar, karaoke
   * already-revealed text. */
  accent: string;
  /** Tint laid over the cover image for legibility. */
  overlay: string;
  /** CSS font stack for text components. */
  fontFamily: string;
  /** Font weight for headlines — separated so themes can pick a
   * lighter or heavier feel. */
  headingWeight: number;
}

const PRESETS: Record<string, Theme> = {
  // Default. Slate background, warm amber accent. Reads as
  // "audiobook" — the palette we used for early demos.
  library: {
    preset: 'library',
    background: '#0F172A',
    primary: '#F8FAFC',
    secondary: '#CBD5E1',
    accent: '#F59E0B',
    overlay: 'rgba(15, 23, 42, 0.55)',
    fontFamily: '"Source Serif 4", "Source Serif Pro", Georgia, serif',
    headingWeight: 700,
  },
  // Warmer, cream-tinged. Pairs with cover art that leans literary
  // / historical fiction.
  parchment: {
    preset: 'parchment',
    background: '#1F1A14',
    primary: '#FBF7F0',
    secondary: '#D6C7A1',
    accent: '#C2410C',
    overlay: 'rgba(31, 26, 20, 0.50)',
    fontFamily: '"EB Garamond", "Garamond", Georgia, serif',
    headingWeight: 600,
  },
  // High-contrast, editorial. Sans-serif. Reads as "non-fiction" —
  // pairs with the abstract / geometric covers we generate for
  // technical or business books.
  minimal: {
    preset: 'minimal',
    background: '#0A0A0A',
    primary: '#FFFFFF',
    secondary: '#A3A3A3',
    accent: '#FAFAFA',
    overlay: 'rgba(10, 10, 10, 0.62)',
    fontFamily: '"Inter", "Helvetica Neue", Arial, sans-serif',
    headingWeight: 800,
  },
};

export const DEFAULT_PRESET = 'library';

/** Resolve a theme by preset name, with optional overrides for the
 * accent and primary colours coming from the SceneSpec. Unknown preset
 * names fall back to the default rather than failing — a stale spec
 * should still render. */
export function resolveTheme(
  preset: string | undefined,
  primaryOverride?: string | null,
  accentOverride?: string | null,
): Theme {
  const base = PRESETS[preset ?? DEFAULT_PRESET] ?? PRESETS[DEFAULT_PRESET];
  return {
    ...base,
    primary: primaryOverride ?? base.primary,
    accent: accentOverride ?? base.accent,
  };
}
