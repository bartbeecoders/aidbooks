// Mirror of `backend/api/src/animation/spec.rs::SceneSpec`.
// Bump `EXPECTED_VERSION` here whenever the Rust constant
// `SCENE_SPEC_VERSION` changes — the cli enforces a strict version
// match so a stale renderer can't silently produce wrong output.

import { z } from 'zod';

export const EXPECTED_VERSION = 1;

const themeSchema = z.object({
  preset: z.string(),
  primary: z.string().nullable().optional(),
  accent: z.string().nullable().optional(),
});

const backgroundSchema = z.discriminatedUnion('kind', [
  z.object({
    kind: z.literal('color'),
    color: z.string(),
  }),
  z.object({
    kind: z.literal('image'),
    src: z.string(),
    kenburns: z.boolean().optional().default(false),
  }),
]);

const sceneSchema = z.discriminatedUnion('kind', [
  z.object({
    kind: z.literal('title'),
    start_ms: z.number().int().nonnegative(),
    end_ms: z.number().int().nonnegative(),
    title: z.string(),
    subtitle: z.string().nullable().optional(),
  }),
  z.object({
    kind: z.literal('paragraph'),
    start_ms: z.number().int().nonnegative(),
    end_ms: z.number().int().nonnegative(),
    text: z.string(),
    tile: z.string().nullable().optional(),
    highlight: z.string().optional().default('karaoke'),
  }),
  z.object({
    kind: z.literal('outro'),
    start_ms: z.number().int().nonnegative(),
    end_ms: z.number().int().nonnegative(),
    title: z.string(),
    subtitle: z.string().nullable().optional(),
  }),
]);

export const sceneSpecSchema = z.object({
  version: z.number().int(),
  chapter: z.object({
    number: z.number().int().nonnegative(),
    title: z.string(),
    duration_ms: z.number().int().positive(),
  }),
  audio: z.object({
    wav: z.string(),
    peaks: z.string().nullable().optional(),
  }),
  theme: themeSchema,
  background: backgroundSchema,
  scenes: z.array(sceneSchema),
  captions: z
    .object({
      src: z.string(),
      burn_in: z.boolean().optional().default(false),
    })
    .nullable()
    .optional(),
  output: z.object({
    mp4: z.string(),
    width: z.number().int().positive(),
    height: z.number().int().positive(),
    fps: z.number().int().positive(),
  }),
});

export type SceneSpec = z.infer<typeof sceneSpecSchema>;
export type Scene = z.infer<typeof sceneSchema>;
export type Background = z.infer<typeof backgroundSchema>;

export function parseSpec(raw: unknown): SceneSpec {
  const spec = sceneSpecSchema.parse(raw);
  if (spec.version !== EXPECTED_VERSION) {
    throw new Error(
      `SceneSpec version mismatch: got ${spec.version}, renderer expects ${EXPECTED_VERSION}`,
    );
  }
  return spec;
}
