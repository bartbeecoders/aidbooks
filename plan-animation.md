# Animation Feature — Plan

> Status: in progress on `feature/animation`. Last updated 2026-05-01.
> Original brief: [`Vibecoding/animation-feature.md`](Vibecoding/animation-feature.md). Master phase plan: [`plan.md`](plan.md).

## Status Tracker

| Phase | State | Notes |
|-------|-------|-------|
| A — Skeleton & contract       | ✅ Shipped (uncommitted on `feature/animation`) | All 8 sub-tasks done, 6 unit tests green, +131 / -0 across 9 files |
| B — Scene planner             | ✅ Shipped (uncommitted on `feature/animation`) | Timing helpers lifted, paragraph-aware planner with tile match + long-paragraph subdivide, 23 animation tests green |
| C — Visual scene library      | ✅ Shipped (uncommitted on `feature/animation`) | 5 components + 3 themes + scene orchestrator. End-to-end render pipeline working (Chromium + ffmpeg + mux). One known duration discrepancy chased + fixed in Phase D. |
| D — YouTube publisher         | ✅ Shipped (uncommitted on `feature/animation`) | Phase C duration bug fixed (variables API + Vite `fs.allow`); paragraph tiles loaded from DB; `animate: bool` plumbed through PublishYoutubeRequest → publication payload → publisher; single-mode concat path; playlist-mode reuses ch-N.video.mp4 directly. Clippy-clean on the new code, 67 workspace tests green. |
| E — Frontend                  | ✅ Shipped (uncommitted on `feature/animation`) | `chapter_video` stream endpoint + frontend API helper + AnimationSection on BookDetail with per-chapter status / progress / inline preview + theme picker (3 presets, end-to-end through to the renderer) + publish-dialog "Use animated video" toggle gated on all chapters being rendered. |
| F — Polish (Whisper, GC, …)   | 🚧 in progress (perf slice) | F.1a/b/c/d/e/f.1 shipped 2026-05-02 (long-lived sidecar, fps default, ffmpeg fast path, concurrency knob, scene_spec_hash cache, ffmpeg hwenc auto-detect). F.1f.2/3 (Chromium GPU) deferred — needs Revideo upstream support. F.2 quality/ops still open. |
| G — Manim diagram path (STEM) | ✅ trunk shipped | G.1–G.6 shipped 2026-05-02. STEM-flagged books with fast path on now route diagram paragraphs through Manim and prose paragraphs through fast-path; ffmpeg-concats + muxes audio. G.7 (frontend diagram-render badge polish) is the only remaining piece. |

> Goal: every audiobook chapter gets an automatically-generated animated video track that runs in lockstep with the narration WAV. The animated MP4 replaces (or composes with) the static cover image used today and becomes the visual layer of the YouTube upload.

## 1. Brief

Today the YouTube publisher (`backend/api/src/jobs/publishers/youtube.rs`) muxes a still cover image + chapter WAVs into an MP4. The result is a "podcast as video" — wholly auditory, visually inert.

We want a **scene-based animated video** generated per chapter:

- **Inputs (already in the system):** chapter `body_md`, narration `ch-N.wav`, `ch-N.waveform.json`, cover image, paragraph illustrations under `<chapter_art>/paragraph-tiles/`, generated subtitles.
- **Output:** `ch-N.video.mp4` (same duration as `ch-N.wav`, 1080p `1920×1080`, 30 fps, AAC stereo) and a per-book `youtube.mp4` made by concatenation.
- **Quality bar:** 1) text on screen tracks the narrator (paragraph-level karaoke), 2) per-paragraph illustration crossfades, 3) subtle waveform-reactive accent (driven by `ch-N.waveform.json`), 4) chapter title cards, 5) book cover bookends. No 3D, no character animation; we are building a *kinetic typography + image-collage* video, not a Pixar short.
- **Constraint:** must be commercial-SaaS-friendly licensing, headless on a Linux render host, driven from a JSON scene spec produced by a Rust job.

## 2. Tool Survey

Researched: **Motion Canvas, Reanimate, Manim CE, Remotion, Revideo, Noon, MoviePy, FFmpeg + gl-transitions.**

| Tool | Headless render | JSON-driven authoring | Audio sync (WAV + peaks) | Captions | License | Ecosystem fit | Total |
|---|---|---|---|---|---|---|---|
| **Remotion** | 5 | 5 | 5 | 5 | **2** (paid Company License for SaaS) | 5 | **27** |
| **Revideo** | 5 | 5 | 4 | 3 | **5** (MIT) | 4 | **26** |
| **Manim CE** | 4 | 4 | 4 (`manim-voiceover`) | 4 | 5 (MIT) | 3 | 24 |
| **MoviePy** | 4 | 4 | 4 | 3 | 5 (MIT) | 3 | 23 |
| **FFmpeg baseline** | 5 | 3 | 4 | 4 | 4 (LGPL build) | 4 | 24 |
| **Motion Canvas** | 2 | 2 | 2 | 3 | 5 (MIT) | 3 | 17 |
| **Reanimate** | 4 | 2 | 2 | 4 | 5 (BSD-3) | 1 | 18 |
| **Noon** | — abandoned (`yongkyuns/noon`, last commit 2022, "no longer actively maintained") | | | | | | DNF |

### Disqualifications
- **Noon** — explicitly abandoned experimental Rust project. Out.
- **Reanimate** — Haskell toolchain in our render image is operational tax we don't want. Out.
- **Motion Canvas (vanilla)** — no first-class headless render path; the community is largely on Revideo for this reason. Use *Revideo* instead.
- **Remotion** — technically the strongest tool; declarative React, best `inputProps` JSON contract, `useAudioData` / `visualizeAudio` map directly onto our existing waveform peaks, official SSR + Docker. **But** for commercial SaaS use it requires a paid Remotion Company License (≈ $100/mo minimum, usage-tiered). Keep on the bench as a paid-tier upgrade path; do not block Phase 1 on it.

### Decision

- **Primary scene engine: Revideo** (MIT, Node + WebCodecs, fork of Motion Canvas with a real `renderVideo({ variables })` API and `<Audio/>` driving scene timing). Pinned to **`@revideo/{2d,core,renderer}@0.10.4`** (current `latest` dist-tag as of 2026-05-01); a stray `1.6.0` exists on npm but is not the maintained line. Renderer service vendored in `backend/render/`.
- **Final compositor / encoder: FFmpeg** (already in the pipeline) — concat per-chapter MP4s, mux audio, burn captions, encode H.264 + AAC.
- **Optional accelerator: Manim CE**, gated by a per-chapter "diagram" scene type — only used when a chapter contains explanatory/diagrammatic content. Out of scope for v1.

## 3. Architecture

```
                ┌────────────────────────────────────────────────────────┐
                │                  Rust backend (Axum)                   │
                │                                                        │
   API ▶────────┤  POST /audiobook/:id/animate                           │
                │  POST /audiobook/:id/publish/youtube?animate=true      │
                │                                                        │
                │  ┌──────────────────────────────────────────────────┐  │
                │  │          jobs::publishers::animate               │  │
                │  │                                                  │  │
                │  │  1. plan_scenes(chapter)  ──► SceneSpec (JSON)   │  │
                │  │  2. spawn revideo render (ipc: stdin = JSON,     │  │
                │  │     stdout = progress NDJSON, file = mp4)        │  │
                │  │  3. probe duration, validate vs WAV duration     │  │
                │  │  4. write storage/<id>/<lang>/ch-N.video.mp4     │  │
                │  └────────────────┬─────────────────────────────────┘  │
                └───────────────────┼──────────────────────────────────-─┘
                                    │ stdin: JSON SceneSpec
                                    │ stdout: {"frame":N,"total":M}
                                    ▼
                ┌─────────────────────────────────────────────────────-──┐
                │  Node renderer  (backend/render, Revideo)              │
                │  $ node dist/cli.js < scene.json > progress.ndjson     │
                │                                                        │
                │  - Loads `project.tsx` with variables = SceneSpec      │
                │  - <Audio src="…/ch-N.wav"/>      ◄── drives timing    │
                │  - <WaveformPulse peaks="…/ch-N.waveform.json"/>       │
                │  - <Paragraphs items={…}/>        ◄── karaoke text     │
                │  - <Tiles src={…paragraph-tiles}/>                     │
                │  - renderVideo() → ch-N.video.mp4                      │
                └────────────────────────────────────────────────────────┘
                                    │
                                    ▼
                ┌────────────────────────────────────────────────────────┐
                │  jobs::publishers::youtube  (existing)                 │
                │  - if animate=true, concat ch-N.video.mp4 instead of   │
                │    looping cover.png                                   │
                │  - mux subtitles.srt as soft track (already produced)  │
                │  - upload to YouTube                                   │
                └────────────────────────────────────────────────────────┘
```

### Why a sidecar Node process and not a long-running service?
- Each render is a one-shot CPU job; the job worker already enforces concurrency.
- Crash isolation: a misbehaving scene file (OOM, infinite loop) is contained in the child process.
- Trivial to swap engines later (Remotion / Manim) — the contract is `stdin: JSON, stdout: NDJSON progress, $1: output path`.

### SceneSpec JSON contract (v1)

```jsonc
{
  "version": 1,
  "chapter": { "number": 3, "title": "The Trust Stack", "duration_ms": 487120 },
  "audio": { "wav": "/abs/path/ch-3.wav", "peaks": "/abs/path/ch-3.waveform.json" },
  "theme": { "preset": "library", "primary": "#0F172A", "accent": "#F59E0B" },
  "background": { "kind": "image", "src": "/abs/path/cover.webp", "kenburns": true },
  "scenes": [
    {
      "kind": "title",
      "duration_ms": 4000,
      "title": "Chapter 3",
      "subtitle": "The Trust Stack"
    },
    {
      "kind": "paragraph",
      "start_ms": 4000,
      "end_ms": 28500,
      "text": "…",
      "tile": "/abs/path/paragraph-tiles/p1.webp",
      "highlight": "karaoke"
    }
    // … one paragraph scene per <p> in body_md
  ],
  "captions": { "src": "/abs/path/ch-3.srt", "burn_in": false }
}
```

`plan_scenes()` (Rust) is responsible for: parsing `body_md` into paragraphs, allocating each paragraph a time window proportional to its character count against the WAV duration (we already do this in `youtube/subtitles.rs` — reuse the function), and pairing it with the matching `paragraph-tiles/p{n}.webp` if available.

## 4. Phased Plan

The feature lives on **`feature/animation`** branched from `main`. Each phase ends with a compilable, runnable, demoable state — no half-finished phases.

### Phase A — Skeleton & contract ✅ (shipped 2026-05-01)

**Goal:** end-to-end shell that turns a fixture chapter into a 10-second placeholder MP4.

- [x] Branch `feature/animation` cut from `main` at `7b160cc`.
- [x] `backend/render/` (Node 20 + TS) with `@revideo/{2d,core,renderer}@0.10.4`, `zod@4.4.1`, TS 6.0.3. `package-lock.json` checked in.
- [x] `backend/render/package.json` scripts: `build` (`tsc -p tsconfig.json`), `cli` (`node dist/cli.js`), `dev` (`tsx src/cli.ts`), `lint`.
- [x] `backend/render/src/cli.ts` reads JSON from stdin, validates against the `EXPECTED_VERSION = 1` zod schema, lazy-imports `@revideo/renderer`, emits NDJSON `{type, ...}` progress on stdout. Smoke-tested against empty stdin / invalid JSON / version mismatch — all rejected with exit 2.
- [x] Placeholder `src/scene.tsx` + `src/project.tsx` — full-bleed cover image (or solid-colour fallback), darkening overlay, `Chapter N` + chapter title fades in, "ListenAI — Animation Preview" footer, `<Audio play/>` for the duration. No paragraph logic yet.
- [x] `backend/api/src/animation/` with `mod.rs`, `spec.rs` (`SceneSpec` + `RenderEvent` mirroring the JSON contract; `SCENE_SPEC_VERSION = 1`), `planner.rs` (Phase A planner: `Title` → `Paragraph(body)` → `Outro`, with proportional collapse on chapters < 8 s).
- [x] `JobKind::Animate` (parent) + `JobKind::AnimateChapter` (worker) in `backend/core/src/domain/job.rs`, with `as_str` / `parse` updated and worker pool sizes (1 + 2) added in `backend/jobs/src/runtime.rs`.
- [x] `backend/api/src/jobs/publishers/animate.rs`:
  - `AnimateParentHandler` enumerates chapters in the requested language, fans out one `AnimateChapter` child per chapter, polls + aggregates terminal status into hub-level `rendering` progress.
  - `AnimateChapterHandler` loads chapter row + duration, validates the WAV exists, builds a `SceneSpec`, drives the Node sidecar over stdin/stdout (NDJSON → `ctx.progress`), folds renderer-side errors into `JobOutcome::Fatal`, and writes `<storage>/<id>/<lang>/ch-<n>.video.mp4`.
  - `animate_mock = true` shortcut path — ffmpeg `lavfi color=black` keeps CI / dev fast without Node.
- [x] `POST /audiobook/:id/animate` in `backend/api/src/handlers/audiobook.rs` (gated on `audio_ready`, supported language, no live `Animate` job), wired in `app.rs` and `openapi.rs`.
- [x] Three new config knobs in `backend/core/src/config.rs`: `animate_node_bin` (default `node`), `animate_renderer_cmd` (empty = refuses to run, like `ffmpeg_bin`), `animate_mock` (default `false`).
- [x] 6 unit tests in `animation::{planner,spec}` covering long-chapter windows, short-chapter collapse, missing-cover fallback, output-path layout, JSON round-trip, NDJSON event parsing — all green.

**Files changed (focused diff: 9 files modified, +131 / -0; 4 new top-level paths):**
- Modified: `backend/api/src/{app,main,openapi,handlers/audiobook,jobs/handlers,jobs/publishers/mod}.rs`, `backend/core/src/{config,domain/job}.rs`, `backend/jobs/src/runtime.rs`.
- New: `backend/api/src/animation/`, `backend/api/src/jobs/publishers/animate.rs`, `backend/render/`, `plan-animation.md`.

**Verification:** `cargo check` clean; `cargo test --bin listenai-api animation::` → 6 passed; `cd backend/render && npm install && npm run build` → clean `dist/{cli,events,spec}.js`; `echo '{...}' | node dist/cli.js` rejects malformed input with structured NDJSON error.

**Known caveats / Phase B follow-ups discovered during Phase A:**
- The Revideo project's `file://` asset path may need a `LISTENAI_ANIMATE_RENDERER_CMD` wrapper in some sandboxed Chromium environments (called out in `backend/render/README.md`). The contract is intentionally swappable — the renderer is `JSON in, MP4 out`, nothing else.
- The Phase A planner's `strip_markdown_minimal` is a one-off; Phase B will replace it with the lifted helper from `youtube/subtitles.rs` (see Phase B sub-tasks).
- `RenderEvent::Done.{mp4, duration_ms}` and `SceneSpec::with_captions` are in the contract but not yet read on the Rust side (`#[allow(dead_code)]`); Phase B's duration tolerance check + Phase D's caption mux will pick them up.

**Done when** *(original):* `just dev-backend` + `cd backend/render && npm run build` then `POST /audiobook/:id/animate` produces `storage/<id>/<lang>/ch-1.video.mp4` whose duration matches `ch-1.wav` ± 50 ms.

**Done when** *(actual):* end-to-end skeleton in place with mock-mode renderer producing a black-frame MP4 of the correct duration. The real Revideo render path is wired but unverified against a live audiobook — first real render is the entry-point of Phase B's hand-off test.

### Phase B — Scene planner ✅ (shipped 2026-05-01)

**Goal:** real per-paragraph timing.

- [x] Lifted `strip_markdown`, `split_sentences`, and `ratio_to_ms` out of `youtube/subtitles.rs` into `backend/api/src/animation/timing.rs`. `subtitles.rs` now imports them — single source of truth, no drift. The lift dropped 121 lines from `subtitles.rs` and added an explicit `PARAGRAPH_MERGE_THRESHOLD` constant matched to `generation::paragraphs::split::MIN_PARAGRAPH_CHARS`.
- [x] New animation-flavoured `split_paragraphs(body_md)` in `timing.rs` that **keeps** sub-threshold blocks (merges them into the predecessor) so the timeline never gaps — distinct from `generation::paragraphs::split`, which **drops** them because image-gen has different invariants. Lone short opener is kept on its own (no predecessor to merge into).
- [x] Replaced the Phase A single-paragraph fallback in `planner::plan` with paragraph-aware allocation:
  - Title (4 s) + paragraph window + Outro (3 s); short chapters (< 8 s) collapse 40/40/20.
  - Paragraph window is split across paragraphs proportional to `chars`, walked cumulatively so rounding never pushes past the window's right edge.
  - Last paragraph's `end_ms` is pinned to the window edge; a final seam-fix pass snaps any 1-ms rounding gaps so the timeline is provably contiguous.
- [x] Long-paragraph subdivision (`MAX_PARAGRAPH_SCENE_MS = 60 s`):
  - Sentence-aware path: groups sentences into `~ total_chars / target_count` buckets, then anchors first/last sub-scene to the parent window edges.
  - Time-split fallback when `split_sentences` returns a single chunk (one big run-on without terminators).
- [x] Tile attachment via [`ParagraphTile { text, image_path }`] passed in by the publisher. Match is substring-containment in either direction (handles minor LLM-pass text drift); no DB calls in the planner. Publisher passes `vec![]` for now — Phase D wires the real lookup against `chapter.paragraphs[].image_paths`.
- [x] 12 planner tests + 9 timing tests covering: long-chapter default windows, short-chapter (< 30 s) collapse, contiguous timeline + duration anchor, missing-cover fallback, image-cover background, tile attach on text overlap, no-attach on mismatch, long-paragraph subdivision, sentence-less time-split fallback, empty-body handling, output-path layout, all the timing-helper edge cases.

**Files changed:**
- New: `backend/api/src/animation/timing.rs` (271 lines incl. 9 tests).
- Rewritten: `backend/api/src/animation/planner.rs` (now 481 lines incl. 12 tests; was 224).
- Modified: `backend/api/src/youtube/subtitles.rs` (-121 lines, now imports from `animation::timing`); `backend/api/src/animation/mod.rs` (`pub mod timing;`); `backend/api/src/jobs/publishers/animate.rs` (passes `paragraph_tiles: Vec::new()`).

**Verification:** `cargo test --bin listenai-api animation::` → 23 passed; `cargo test --workspace` → 67 passed (was 50 before Phase B); `cargo clippy --bin listenai-api --all-targets -- -D warnings` clean on the animation modules (pre-existing warnings in `generation/translate.rs`, `youtube/subtitles.rs::cues_cap_at_max_length` test, `llm/openrouter.rs` unaffected).

**Phase C entry point:** the Node sidecar's `scene.tsx` now receives a richer spec (one or more `Paragraph` scenes with karaoke text + optional tile path) instead of the Phase A "single Paragraph(body)" fallback. The placeholder renderer ignores the new data; Phase C's job is to consume it.

**Done when** *(original):* snapshot tests pass and a manually-inspected `SceneSpec` for the demo audiobook makes sense (eyeball `start_ms` vs the actual narration in Audacity).

**Done when** *(actual):* timeline-invariant tests and per-feature snapshot tests pass; the Audacity eyeball check is deferred to the first Phase C real render (it requires a non-mock `LISTENAI_ANIMATE_RENDERER_CMD` running against an actual audiobook, which is more tractable once Phase C's scene library can show what the planner output looks like).

### Phase C — Visual scene library ✅ (shipped 2026-05-01)

**Goal:** the placeholder Revideo project becomes a real scene library.

Components in `backend/render/src/components/`:
- [x] `Background.tsx` — Ken Burns slow zoom (1.00 → 1.10 scale) + ~40 px horizontal drift over the chapter duration, theme overlay tint baked on top. Solid-colour fallback when no cover image.
- [x] `TitleCard.tsx` — "Chapter N" eyebrow + chapter title + accent underline that draws left → right during fade-in. Subtitle optional. Reads from theme for fonts + colours.
- [x] `ParagraphScene.tsx` — left tile (720×720 with rounded clip) crossfades up, right karaoke text reveal driven by a `revealedChars` signal advancing linearly through the scene's hold window. Two stacked Txts (dim baseline + accent overlay) so wrap stays stable. Falls back to text-only when no tile is provided.
- [x] `WaveformPulse.tsx` — 96-bar accent strip across the bottom; bars driven by squared peaks from `ch-N.waveform.json` with a small phase fan-out across bars. Lazy-fetches the peaks JSON; missing or malformed file logs to stderr and renders a flat strip rather than failing.
- [x] `Outro.tsx` — title + accent underline + optional subtitle, structurally mirrors `TitleCard` so the chapter's bookends feel consistent.

Themes in `backend/render/src/themes/index.ts` — three presets (`library`, `parchment`, `minimal`) plus a `resolveTheme(preset, primaryOverride, accentOverride)` helper that always returns a valid theme (unknown preset names fall back to default rather than throwing). Each preset specifies background, primary, secondary, accent, overlay, fontFamily, headingWeight.

**Orchestrator** (`scene.tsx`):
- Reads the full `SceneSpec` from `useScene().variables.spec` (one structured variable, set by `cli.ts`).
- Builds the static node tree: Background → all cards → WaveformPulse → Audio.
- Walks `spec.scenes[]` with a timeline cursor, yielding each card's `show(durationSec)` at the right time. Each card owns its own intro/hold/outro fade timing (`Math.min(0.5, dur/4)` for fades).
- Background and WaveformPulse run in parallel via `all()` for the full chapter duration.

**CLI** (`cli.ts`):
- Rewrites every host path inside the spec (`audio.wav`, `audio.peaks`, `background.src`, `scenes[].tile`) into `file://` URLs and passes the whole structured spec as a single `variables.spec` entry.
- Splits the absolute output path into `outDir` + `outFile` (Revideo expects a bare filename ending in `.mp4`), creates `outDir` if missing, and points Revideo at it.
- Bridges Revideo's `progressCallback(workerId, 0..1)` into our NDJSON `frame` events keyed off `(duration_s × fps)` total frames.

**Build / verification:**
- New build artifact: `npm run lint` (`tsc -p tsconfig.check.json`) typechecks every TSX file as well as the CLI driver. Phase A's build only typechecked the CLI; Phase C catches scene-tree errors before they surface in Chromium.
- New deps: `@revideo/ui` and `@revideo/vite-plugin` (peer dependencies of `@revideo/renderer` that we missed in Phase A) plus `vite@5.4.21`.
- End-to-end smoke (2 s test WAV + minimal SceneSpec) reaches a valid 1920×1080 H.264 + 48 kHz AAC MP4 on disk; full pipeline runs (validation → Chromium boot → scene render → ffmpeg concat + mux + cleanup).

**Files changed:**
- New: `backend/render/src/themes/index.ts` (3 presets + `resolveTheme`); `backend/render/src/components/{Background,TitleCard,ParagraphScene,WaveformPulse,Outro,types}.tsx` (5 components + shared `Card` interface); `backend/render/tsconfig.check.json` (lint config that includes TSX).
- Rewritten: `backend/render/src/scene.tsx` (Phase A placeholder → orchestrator); `backend/render/src/cli.ts` (single-`spec` variable, `outDir/outFile` split, `progressCallback` bridge).
- Modified: `backend/render/package.json` (`@revideo/ui`, `@revideo/vite-plugin`, `vite` deps; `lint` script repointed at the check config).

**Known caveat — duration discrepancy (Phase D will fix):**
- The render pipeline produces a syntactically-valid MP4 but the encoded video stream is currently truncated to ~1.07 s regardless of input duration (the audio stream is full-length). Revideo's `progressCallback` fires for the full expected frame count, but only ~32 frames land in the MP4. This is almost certainly a scene-duration-vs-encoder-rate misalignment in either my generator structure or a Revideo setting we haven't found yet. The visual layout, fonts, audio mux, and ffmpeg encoder all work — the issue is purely the scene's measured frame count. Phase D's first task is reproducing this against a real audiobook and tracing it; the fix is likely a `scene2D` setting or a tweak to how the orchestrator structures `all(...)`.

**Done when** *(original):* rendering the demo book produces a video that is *watchable* end-to-end (a human reviewer wouldn't turn it off in the first 30 seconds). Document the visual-quality bar with side-by-side screenshots in `docs/animation/v1-quality-bar.md`.

**Done when** *(actual):* scene library shipped + render pipeline runs end-to-end + `npm run lint` clean across all TSX. The "watchable demo book" eyeball test is gated on resolving the duration discrepancy, which moves into Phase D. The screenshots doc gets written once the first real-asset render lands.

### Phase D — Integration with the YouTube publisher ✅ (shipped 2026-05-01)

**D.1 — Phase C duration bug, traced + fixed.** The encoded video was truncating to ~1.07s regardless of input duration. Root cause was two stacked issues:
- The Revideo `variables` API is **signal-based** — `useScene().variables.get<T>(name, default)` returns a `() => T` getter. Phase C's scene was reading `(variables as unknown as SpecVars).spec` which is *always* undefined, so it silently fell through to the synthetic 1-second `FALLBACK_SPEC`. Revideo dutifully rendered 1 second.
- Once that was fixed, `<Audio>` / `<Img>` components 404'd because Vite's default `server.fs.allow` restricts file access to the project root. Two fixes: open `viteConfig.server.fs.allow` to `/` (the renderer runs on the backend host, so any path the worker can read is fine), and rewrite host paths to `/@fs<absolute>` URLs instead of `file://` (Chromium can't fetch `file://` from a page served over HTTP).
- End-to-end smoke (6s spec): 182 frames at 30fps, full 6.07s duration, valid 1920×1080 H.264 + 48kHz stereo AAC. The "watchable demo book" eyeball test from Phase C's `Done when` is now achievable (a private real-asset render is the entry point of Phase E).

**D.2 — Paragraph tiles wired from `chapter.paragraphs[].image_paths`.** New `load_paragraph_tiles(state, audiobook_id, chapter_number)` in `publishers/animate.rs`:
- Always reads from the **primary** language's chapter row (translations share the same image set, anchored to the primary).
- Picks the first non-empty `image_paths[ordinal-1]` per paragraph, resolves it against `storage_path`, attaches it as a `ParagraphTile { text, image_path }` for the planner's substring match.
- Errors degrade silently to `Vec::new()` — a missing tile means the planner emits a text-only scene for that paragraph, never a job failure.

**D.3 — `animate: bool` plumbed through the publish flow.** Added `animate: Option<bool>` (default `false`) to:
- `PublishYoutubeRequest` (the HTTP body).
- The publication-job payload that `PublishYoutubeHandler` reads.
- HTTP-side pre-check: new `first_missing_animation(state, audiobook_id, language)` returns the first `ch-N.video.mp4` not present on disk; the handler 409s with a clear "run POST /audiobook/:id/animate first" error rather than enqueueing a publish job that's guaranteed to fail.
- Publisher-side defense in depth: a stale job whose chapter MP4s have been GC'd between enqueue and pickup also fails with the same Conflict error.
- Shorts (`is_short = true`) + `animate = true` → 409: the 9:16 Short composite can't blend with our 16:9 chapter renders. Phase F can revisit.

**D.4 — Single-mode `concat` path.** In `publishers/youtube.rs::run_single`, when `animate=true`:
- Skip the slideshow build (image segments + chapter WAVs + ffmpeg encode).
- Verify every `ch-N.video.mp4` exists on disk (else fail Conflict).
- Call new `concat_animated_chapters(state, &chapter_videos, &mp4_path)` which writes a concat list file, runs `ffmpeg -f concat -safe 0 -i list.txt -c copy -movflags +faststart out.mp4`. No re-encode — ~free on multi-GB inputs since the chapter renders are already H.264 + AAC at the same params.
- Subtitles continue to be uploaded as a separate caption track via `upload_book_captions` (no muxing change).

**D.5 — Playlist-mode integration.** In `run_playlist` and `run_playlist_preview`, when `animate=true`:
- Skip the per-chapter encode step entirely.
- Use `<storage>/<book>/<lang>/ch-N.video.mp4` as the per-chapter upload source directly (the renderer already produced complete clips per chapter — no downstream encoding needed).
- Resume support and per-chapter error persistence stay intact.

**Files changed:**
- Modified: `backend/api/src/handlers/integrations.rs` (`animate: Option<bool>` on `PublishYoutubeRequest`, `first_missing_animation` pre-check, payload field).
- Modified: `backend/api/src/jobs/publishers/youtube.rs` (`animate` payload read; `run_single` / `run_playlist` / `run_playlist_preview` get `animate: bool`; `run_single` branches on it; new `concat_animated_chapters` ffmpeg helper).
- Modified: `backend/api/src/jobs/publishers/animate.rs` (`load_paragraph_tiles` + threading into `PlanInput`).
- Modified: `backend/render/src/scene.tsx` (signal-based `variables.get('spec', FALLBACK_SPEC)` instead of broken `.spec`-property read).
- Modified: `backend/render/src/cli.ts` (path → `/@fs<abs>` URL rewrite + `viteConfig.server.fs.allow=['/']`).

**Verification:**
- `cargo check` clean across the workspace.
- `cargo test --workspace` → 67 passed (no regressions from Phase A/B/C; no new tests needed since D mostly rewires existing flows).
- `cargo clippy --bin listenai-api --all-targets -- -D warnings` clean on every animation/D file (5 pre-existing clippy hits in `generation/outline.rs`, `generation/translate.rs`, `llm/openrouter.rs` are unaffected).
- End-to-end render smoke: 6s test spec produces a 6.07s MP4 with both video and audio streams in the right format + duration.

**Done when** *(original):* `POST /audiobook/:id/publish/youtube?animate=true` produces a YouTube-ready MP4 whose video track is the animated scenes and audio track is unchanged. Verify with a real upload to a private/unlisted test channel.

**Done when** *(actual):* the request flag is wired end-to-end, both single + playlist modes have a concat-only path, and the smoke test produces a structurally-correct MP4. The "real upload to a private channel" verification is gated on Phase E (frontend toggle + a real audiobook with a finished animate run) — no point burning a YouTube quota slot just for the publisher unit test.

### Phase E — Frontend ✅ (shipped 2026-05-01)

**E.1 — Backend stream endpoint.** New `GET /audiobook/:id/chapter/:n/video` in `handlers/stream.rs::chapter_video`. Streams `<storage>/<book>/<lang>/ch-N.video.mp4` with `Content-Type: video/mp4`, `Accept-Ranges: bytes`, and a real `Content-Length` so the `<video>` element can scrub. 404 until the animate job has produced the file. Same `?language=&access_token=` auth pattern as `chapter_audio`. Wired in `app.rs` + `openapi.rs`.

**E.2 — Frontend API client.** Added in `frontend/src/api/`:
- `audiobooks.animate(id, { language, theme, idempotencyKey })` → `POST /audiobook/:id/animate?language=&theme=`.
- `chapterVideoUrl(audiobookId, chapter, accessToken, language?)` URL helper.
- `AnimationTheme` type (`"library" | "parchment" | "minimal"`) re-exported from `api/types.ts`.
- Extended `PublishYoutubeRequest` with `animate?: boolean | null`.

**E.3 — `AnimationSection` on `BookDetail`.** Added between the activity log and the chapters list, only renders when `allAudioReady`:
- Theme picker — three buttons (Library / Parchment / Minimal). Selection plumbs end-to-end: HTTP query → publication-job payload → `AnimateChapter` child payload → `PlanInput.theme_preset` → `SceneSpec.theme.preset` → renderer's `resolveTheme(...)`. Backend rejects unknown presets with a 400 rather than silently falling back.
- Per-chapter rows showing one of: *Not generated* / *Queued* / *Rendering NN%* (with a thin progress bar) / *Ready* / *Failed* (with the `last_error` tail). Status reads from `progress.jobs` filtered by `kind === 'animate_chapter'` and keyed on `chapter_number` — same WebSocket the rest of the page already consumes via `useProgressSocket`, no extra query.
- Inline `<video controls>` preview once a chapter row hits *Ready* — points at the new `chapterVideoUrl` and cache-busts on the job id so a re-render swaps in the new MP4.
- A new "🎬 Animate" button in the top action bar fires `audiobooks.animate(id, ...)`. Disabled until `allAudioReady` and while the parent `Animate` job is in flight.
- *Per-chapter "Re-generate" was deferred to Phase F* — it requires a dedicated `POST /audiobook/:id/chapter/:n/animate` endpoint; the current parent always fans out the full book. Documented inline.

**E.4 — Publish dialog toggle.** New `Use animated video` checkbox on `PublishYoutubeDialog`:
- Disabled until *every* chapter has a `completed` `animate_chapter` job (computed from the same map the section uses).
- Disabled with a different tooltip on Shorts (the backend 409s `is_short=true && animate=true` because of the 9:16↔16:9 mismatch — the UI matches that gate so the user can't even submit it).
- Sends `animate: true` on the publish request body. Phase D's publisher handles the rest.

**Files changed:**
- Modified: `backend/api/src/handlers/stream.rs` (`chapter_video`); `backend/api/src/handlers/audiobook.rs` (`AnimateQuery` + theme validation + payload plumb); `backend/api/src/jobs/publishers/animate.rs` (forward payload theme to children + read in child); `backend/api/src/app.rs` + `openapi.rs` (route registration); `frontend/src/api/{index,types}.ts` (animate call, video URL, theme type, request flag); `frontend/src/pages/BookDetail.tsx` (animate mutation + animation jobs map + AnimationSection / AnimationRow components + action button + dialog toggle).

**Verification:**
- Backend: `cargo check` clean, `cargo test --workspace` → 67 passed (no regressions).
- Frontend: `npm run typecheck` clean for every Phase-E touch (the only TS errors remaining are 2 pre-existing ones in `api/index.ts` and `routes.tsx` unrelated to this work).
- The action button + dialog toggle are wired against the real backend gates: the `/animate` endpoint validates language + theme, the `/publish/youtube` endpoint already 409s on missing animations, and the UI mirrors both states.

**Out-of-scope for v1 (deferred to Phase F):**
- ~~Per-chapter `Re-generate this chapter` — needs a new endpoint (`POST /audiobook/:id/chapter/:n/animate`) so the user can iterate on a single chapter without re-rendering the whole book. The full-book re-render via the existing button works as a workaround.~~ **Shipped 2026-05-02** as part of Phase F: new `POST /audiobook/:id/chapter/:n/animate` endpoint + per-row "Re-generate" button on `AnimationSection`. Endpoint busts the F.1e cache (deletes `<mp4>.hash` and the `.mp4`) before enqueueing a parentless `AnimateChapter` job; frontend mutation invalidates audiobook query and respects the in-flight chapter check (`has_live_animate_chapter`) so the user can't queue overlapping renders.
- A live render-error stderr tail for admins — the publisher already surfaces errors via `JobSnapshot.last_error`, which the row shows; piping the Node sidecar's stderr tail through to the UI for admins is a separate observability piece.

**Done when** *(original):* an end user can, from a fresh audiobook with audio already generated, click "Generate animation" → watch progress → preview each chapter → publish to YouTube with the animated track, all without touching the CLI.

**Done when** *(actual):* every UI surface is wired against the real backend. The end-to-end "real audiobook → real animation → real YouTube upload" exercise still requires a real audiobook + finished animate run + a private YouTube channel — that's the natural Phase F integration test.

### Phase F — Polish (deferrable, separate PR)

> Phase F is split into two slices. **F.1 Speed** is the new headline work — a chapter currently takes roughly 1.5–4× realtime on a Linux host with no GPU; the goal of F.1 is to land at 0.2–0.5× realtime on the same host (5–10 min for a 30 min chapter) and 0.05–0.15× realtime when a GPU is available. **F.2 Quality + ops** is the original Phase F backlog (word timing, cost tracking, quota, cache, GC).
>
> Out-of-band investigation (2026-05-02): we evaluated **x.ai `grok-imagine-video`** as a wholesale replacement for Revideo. It does not fit: max 15 s per generation × "up to several minutes" wall-clock per generation × 15–30 paragraphs per chapter ⇒ 30–120 jobs and *hours* of latency per chapter, before per-second pricing. It also gives up determinism (breaks the `scene_spec_hash` cache below), karaoke text sync, audio cadence sync, and brings hallucination risk on non-fiction. Keep on the bench as an opt-in "live photo" tile generator (still images become 8 s low-motion clips, fed back into the existing tile slot) — but not as the default render path.

#### F.1 — Speed

The render's wall-clock today decomposes roughly as: ~10–30 s Chromium cold start + ~70 % frame raster + ~15 % H.264 software encode + ~5 % ffmpeg concat/mux. Every line below targets one of those numbers.

- [x] **F.1a — Long-lived renderer sidecar.** *(shipped 2026-05-02)* New `backend/render/src/server.ts` reads NDJSON specs from stdin and processes them sequentially, keeping the Revideo / Vite / Chromium context warm between renders. `cli.ts` slimmed to a one-shot wrapper around the shared `render.ts`, so `scripts/animate-single-chapter.sh` keeps working unchanged. Protocol additions: `{type:"ready"}` event emitted at boot and again between renders; `{type:"bye"}` on graceful shutdown when stdin closes. Mirrored on the Rust side as `RenderEvent::Ready` / `Bye`. New `backend/api/src/animation/sidecar.rs` owns a `RendererPool` with `min(animate_concurrency, 4)` slots, each backed by a long-lived `node dist/server.js` child; per-render lifecycle: acquire → write spec line → drain events through an `mpsc::UnboundedSender<RenderEvent>` until `ready` → return slot to pool. Pool recycles a sidecar after `DEFAULT_MAX_RENDERS_PER_PROC = 50` renders OR `DEFAULT_MAX_AGE_SECS = 30 min`, whichever hits first; transient failures (EOF, broken pipe, mid-render `bye`) drop the sidecar and the next acquire spawns a fresh one. `AnimateChapterHandler` lazily constructs the pool on first non-mock render via a `tokio::sync::OnceCell` so mock-mode + missing-cmd error path stay zero-cost. Backwards-compat shim: a configured `dist/cli.js` path auto-maps to `dist/server.js` with a single deprecation log line. 4 new unit tests cover capacity clamping, default config, and the missing-cmd-is-fatal path. Demo script smoke-tested end-to-end (1920×1080@24fps H.264, drift 66 ms over a 5 s spec). The legacy `run_node_render` per-call spawn is gone; mock-mode keeps its inline ffmpeg helper.
- [x] **F.1b — Drop to 24 fps default.** *(shipped 2026-05-02)* `LISTENAI_ANIMATE_FPS` config knob, default 24. Threaded through `Config::animate_fps` → `PlanInput::fps` → `Output::hd_1080(mp4, fps)`. The renamed `hd_1080p30` constructor is gone; `Output::hd_1080(_, 30)` keeps the old behaviour reachable for callers that need it.
- [x] **F.1c — ffmpeg-only fast path.** *(shipped 2026-05-02)* New `backend/api/src/animation/fast_path.rs`: builds an auto-generated ASS subtitle file (`Title` / `Paragraph` / `Outro` styles, theme-aware colours via `palette_for(preset)`) + a `filter_complex_script` (`[0:v]scale,zoompan,format=yuv420p,subtitles='...'[v_out]` for image backgrounds; subtitles-only for colour) and runs **one** ffmpeg invocation per chapter. No Chromium, no Vite, no Node — pure libavcodec + libavfilter. Wired into `AnimateChapterHandler::run` behind `LISTENAI_ANIMATE_FAST_PATH=false` (default off until real-content QA). Cache aware: `cache.rs` now emits two labels (`REVIDEO_PATH_LABEL = "revideo-v1"`, `FFMPEG_PATH_LABEL = "ffmpeg-v1"`) folded into `compute_spec_hash(spec, render_path)`, so flipping the flag invalidates cleanly. Progress is mapped from ffmpeg's `-progress pipe:2` `frame=N` lines into a 0..1 fraction over the same `mpsc::UnboundedSender<f32>` idiom the pool uses, then folded into the same `0.05 → 0.99` envelope as the Revideo path. Sidecar files (`<stem>.fastpath.ass`, `<stem>.fastpath.filter`) live next to the output MP4; cleaned up on success, kept on failure for debugging. v1 trade-offs (intentional, documented): no animated title underline, no per-paragraph tile overlays, no per-word karaoke (full paragraph text fades in/out per cue with `\fad(300,300)`), no waveform pulse, hard cuts between scenes. 11 new unit tests cover ASS colour swap, ASS time format, escape rules, dialogue count, preset fallback, empty-text skipping, and filter-graph variants. One `#[ignore]`'d real-ffmpeg integration test (`renders_a_real_mp4`) renders a 4 s color-background fixture end-to-end in ~0.4 s; runs locally via `cargo test --bin listenai-api -- --ignored fast_path`.
- [x] **F.1d — Parallelism bump.** *(shipped 2026-05-02)* `LISTENAI_ANIMATE_CONCURRENCY` config knob (default `0` = auto: `min(available_parallelism, 4)`). New `WorkerConfig::with_animate_concurrency(n)` builder applied at boot in `main.rs`. Memory budget reminder: each worker holds ~400 MB Chromium RSS, so 4 workers ≈ 1.6 GB before F.1a lands.
- [x] **F.1e — `scene_spec_hash` cache.** *(shipped 2026-05-02)* `backend/api/src/animation/cache.rs`: SHA-256 over the spec (with `output.mp4` zeroed) + a `RENDER_PATH_LABEL` constant + the mtimes of every referenced input (audio WAV, peaks JSON, cover, paragraph tiles, captions). Stored as `<mp4>.hash` next to the artefact. Cache hit → emit `progress("cached", 1.0)` and return Done without spawning the renderer; cache miss → render, then write the hash. 7 unit tests cover stability, output-path independence, fps invalidation, title invalidation, and round-tripping. Cache write failures degrade silently (next run misses cache, doesn't fail the job).
- **F.1f — GPU offload.** Three vectors, layered cheapest-first.
    1. **F.1f.1 — ffmpeg hardware encoder** ✅ *(shipped 2026-05-02)*. New `backend/api/src/animation/hwenc.rs`: `Encoder` enum (`Software` / `Nvenc` / `Vaapi` / `Qsv`), `detect(ffmpeg_bin, override, vaapi_device)` that runs `ffmpeg -hide_banner -encoders` once + sniffs `/dev/nvidiactl` and the configured DRI render node, `encoder_args(encoder)` and `pre_input_args(encoder, vaapi_device)` returning the right argv tail per encoder, `filter_graph_tail(encoder)` returning the `,format=nv12,hwupload` chain VAAPI needs after libass. Wired into `fast_path::render` via two new config knobs: `LISTENAI_ANIMATE_HWENC` (default `auto`) and `LISTENAI_ANIMATE_VAAPI_DEVICE` (default `/dev/dri/renderD128`; override to `renderD129` etc. on hybrid GPU hosts). Detection result cached in a process-local `OnceLock<OnceCell<Encoder>>` so the probe runs once. Three live `#[ignore]`'d integration tests (`renders_a_real_mp4`, `renders_with_nvenc`, `renders_with_vaapi`) exercise software / NVENC / VAAPI end-to-end. 13 hwenc unit tests + 2 fast-path tests cover detection priority (NVENC > VAAPI > QSV > Software), override aliases (`none`/`software`/`cpu`), unknown override falls back to software, GPU device required for HW pick, encoder-args quality knob lands in the right per-encoder arg, VAAPI tail only set for VAAPI, configurable VAAPI device path. Doesn't invalidate the F.1e cache (encoder choice is environment, not content).
    2. **F.1f.2 — Chromium GPU rasterization + WebCodecs hardware H.264** (Revideo path) — **deferred**. Revideo's `renderVideo` API doesn't currently surface puppeteer launch args, so flipping the relevant Chromium flags (`--use-gl=angle --enable-features=Vulkan,WebCodecsHardwareEncoding --ignore-gpu-blocklist --enable-gpu-rasterization`) needs either an upstream patch or a forked launch path. Empirical estimate from the upstream issue tracker is 2–4× on the Revideo path; not enough to warrant a fork ahead of F.2 work. Revisit if F.1c's fast path stalls on real content.
    3. **F.1f.3 — Skia GPU canvas** — covered by F.1f.2 once that lands. No incremental work.

**Speed budget (target on a 30-min chapter, 720p 24 fps):**

| Configuration | Today | Target after F.1 |
|---|---|---|
| Cold-start cost across 12 chapters | 2–6 min | < 30 s (sidecar; F.1a) |
| Per-chapter frame raster | ~3–5 min | ~1.5–2.5 min CPU / ~0.6–1 min GPU (F.1c + F.1f.2) |
| Per-chapter encode | ~30–60 s | ~5–10 s (F.1f.1) |
| Total per chapter (CPU-only) | ~5–8 min | ~2–3 min |
| Total per chapter (GPU host) | n/a | ~45–90 s |

#### F.2 — Quality + Ops

- [ ] **Word-level caption timing** via Whisper-on-existing-WAV (`whisper.cpp` on the host or `groq/whisper-large-v3` over HTTP). Replace the constant-cadence karaoke with real word timing. Cache results next to `ch-N.wav` as `ch-N.words.json` so re-renders don't re-transcribe. Required for F.1c's `subtitles` filter to look right; otherwise the fast path inherits the same drift the Revideo path has today.
- [ ] **Cost & duration tracking**: log render-seconds and CPU-seconds (and GPU-seconds when F.1f is on) to `generation_event` so admins can see the cost-per-minute-of-output.
- [ ] **Quota / gating**: animation is expensive — wire it into the existing quota system on a per-user basis.
- [ ] **GC**: the operational-concerns note about garbage-collecting `ch-N.video.mp4` 30 days after `youtube_url` is set lands here; reuse `JobKind::Gc`.
- [ ] **Optional: grok-imagine-video "live photo" tiles.** Off by default; enabled per book via a `--live-tiles` flag on the animate request. For each `paragraph_tiles[i].image_path`, fire `POST /v1/videos/generations` with the still as `image`, prompt = the paragraph text trimmed to a visual-friendly phrase, `duration=8`, `resolution=720p`, then download the result and use it as a `tile_video` field on the paragraph scene. The Revideo / fast-path scene player loops/freezes the 8 s clip across the paragraph window. Strictly a v1.1 polish — not on the speed-critical path.

### Phase G — Manim diagram render path (STEM-only)

**Goal:** for STEM books (math / physics / chemistry / biology / CS / engineering), per-paragraph diagrams replace bare karaoke text. Manim CE owns these segments; the existing fast path keeps owning prose paragraphs; ffmpeg concats the lot. Non-STEM books are unaffected.

**Architecture (target):** per-segment rendering — title and outro stay on the chosen base path (Revideo or fast path), each `Paragraph` scene routes by `visual_kind`: `prose` → fast path, anything else → Manim. ffmpeg `-c copy` concatenates all per-segment MP4s, then muxes the chapter WAV. Same pattern as the book-level `concat_animated_chapters` already in `publishers/youtube.rs`.

#### G.1 — STEM detection + user override ✅ (shipped 2026-05-02)

- **Migration `0033_audiobook_stem.surql`** — adds `stem_detected: option<bool>` (LLM verdict) + `stem_override: option<bool>` (user toggle) to the `audiobook` table. Effective STEM = `override.unwrap_or(detected.unwrap_or(false))`.
- **Outline LLM prompt extended** — `outline_v1.md` now asks for `is_stem: bool` with a guidance block ("math / physics / chemistry / biology / CS / engineering" + concrete borderline-case examples). `OutlineJson::is_stem` flows into `persist_outline`, which writes `stem_detected` only — `stem_override` is intentionally untouched on rerun so a user choice survives outline regenerates.
- **API surface** — `DbAudiobook` + `AudiobookSummary` carry `stem_detected`, `stem_override`, and a pre-computed `is_stem` (effective). `PATCH /audiobook/:id` accepts `stem_override` as `serde_json::Value` with three-state semantics: absent → don't change, `null` → clear (use detection), `true`/`false` → force the value.
- **Frontend** — new `StemToggle` component renders a 3-button segmented control inside `AnimationSection`: **Auto** (label includes the LLM verdict, e.g. "Auto (STEM)"), **STEM**, **Not STEM**. Wires through `audiobooks.patch({ stem_override })`. The "Effective" trailing indicator shows what the renderer will actually use.
- 0 net behaviour change on the render path today — G.1 is purely the data plumbing + UX. Phase G.2 will start consuming `is_stem` to decide whether to run the diagram path.

#### G.2 — Per-paragraph visual classifier ✅ (shipped 2026-05-02)

- New `PromptRole::ParagraphVisual` enum variant + `paragraph_visual_v1.md` template (seeded). The prompt enumerates 8 allowed `visual_kind`s with template-specific `visual_params` shapes inline (`function_plot`, `axes_with_curve`, `vector_field`, `free_body`, `flow_chart`, `bar_chart`, `equation_steps`, `neural_net_layer`) and ends with a calibration block of concrete examples so the LLM has anchor cases for borderline paragraphs.
- New `paragraphs::extract_visual_kinds(...)` async fn parallel to `extract_scenes`. Same degrade-gracefully posture: load-template / pick-model / chat / parse failures all log a warn and return an empty map. Validates `visual_kind` against `ALLOWED_VISUAL_KINDS` and drops unknown values silently. Hard-caps diagrams at 8 per chapter (lowest-indexed first), keeping the Manim render budget bounded even if the LLM marks every paragraph visual.
- `merge_for_persist` now takes a third `visuals` map and emits `visual_kind` + `visual_params` on the per-paragraph JSON only when present — non-STEM books and prose paragraphs stay slim.
- `ChapterParagraphsHandler` loads `stem_detected` + `stem_override` alongside the title/topic/genre and computes the same effective-STEM fallback the detail endpoint uses. STEM-only second pass runs after `extract_scenes` with a fresh `extracting_visuals` progress label so the UI can distinguish the two LLM calls.
- `DbParagraph` carries optional `visual_kind` + `visual_params` (the latter `#[allow(dead_code)]` until the Manim sidecar in G.5 starts reading it). `ParagraphSummary` exposes `visual_kind` for the frontend; `visual_params` stays server-side until the renderer needs it.
- Frontend: `ParagraphSummary` type augmentation for `visual_kind`; new "📐 N" diagram badge on `AnimationRow` when the chapter has any paragraph with a `visual_kind`. Hover-title explains "N paragraphs will render as a diagram via Manim (Phase G)".
- 2 new unit tests: `merge_includes_visual_kind_when_labelled` covers the persisted shape; `allowed_visual_kinds_is_non_empty` is a cheap sanity check that the const stays populated. Total animation-relevant tests: 97 (was 95).
- **Backfill helper** `POST /audiobook/:id/chapter/:n/classify-visuals` (added 2026-05-02): re-runs `extract_visual_kinds` against an existing chapter's saved paragraphs without rewriting the body. Required because the classifier only fires from `chapter_paragraphs` when the book was STEM at chapter-write time — books generated before that flip stay un-classified until this endpoint runs. Frontend `AnimationRow` shows a "📐 Classify diagrams" button when `isStem && diagramCount === 0`; query invalidation refreshes the badge in place. Backend 400s if the book isn't STEM (no point classifying non-STEM content) or if the chapter has no paragraphs (re-generate the body first).

#### G.3 — Manim toolchain ✅ (shipped 2026-05-02)

- New Python package `backend/manim/listenai-manim` (hatchling-built, `pyproject.toml` + `requirements.txt` + `uv.lock` workflow). Pinned `manim>=0.18,<0.19`; LaTeX kept (user-confirmed) so `MathTex` produces real rendered math.
- `backend/manim/listenai_manim/smoke.py` exercises Pango text + LaTeX `MathTex` + `Axes`/`plot()`/`Create` — every primitive G.4 templates will rely on. Each act has a docstring callout for which dependency a failure points at.
- `backend/manim/Containerfile` (podman-first; works under docker too): `python:3.11-slim-bookworm` base, installs ffmpeg + cairo/pango + `texlive-latex-base/extra/fonts-recommended/science` + `dvisvgm`, pip-installs Manim from `requirements.txt`, drops to a non-root `manim` user, default CMD = smoke. Image weighs in at ~3 GB.
- `docs/animation/render-host.md` documents native install for Arch/EndeavourOS (`pacman + uv venv` — matches the dev box), Debian/Ubuntu, macOS, Fedora, plus the podman flow with `:Z` SELinux relabelling. Operational notes cover image size budget, first-render LaTeX cache cost (5–10 s once), CPU-only encoding, RSS budget, and a troubleshooting block for the four failures most likely to bite (missing `physics.sty`, missing libpango runtime, `dvisvgm` not on PATH, container builds tripping over SELinux).
- New just recipes: `manim-build` (uv sync), `manim-smoke` (renders 4-second MP4), `manim-container-build` (podman build), `manim-container-smoke` (podman run smoke). Mirror the existing `animate-*` pattern.

#### G.4 — Template library ✅ (shipped 2026-05-02)

Eight hand-coded Manim `Scene` subclasses, one per `visual_kind` from `paragraph_visual_v1.md`'s allowed list. Each lives in `backend/manim/listenai_manim/templates/<kind>.py`, subclasses the shared `TemplateScene` (`_base.py`), and reads `self.params` (the LLM's `visual_params`) + `self.run_seconds` (the planner's scene window). The shared `render(scene_cls, params, duration_ms, output_path)` helper handles `tempconfig`, per-render tempdir, and final MP4 placement; sub-templates only own the `construct()` body.

| `visual_kind` | What it draws | Params shape |
|---|---|---|
| `function_plot` | y = f(x) on auto-scaled axes; sandboxed eval over numpy. Optional emphasize callout. | `{ fn, domain, emphasize? }` |
| `axes_with_curve` | Qualitative axes with text labels (no ticks) and a stock curve (linear/exp/log/sin/cos). | `{ x_label, y_label, curve_kind, emphasize? }` |
| `vector_field` | 7×5 grid of arrows, shape (rotational/radial/uniform) inferred from the description string. | `{ description }` |
| `free_body` | Central object + labelled force arrows in conventional directions (gravity↓, normal↑, friction←, …). | `{ object, forces[] }` |
| `flow_chart` | 3–7 boxes auto-laid out in 1 or 2 snake rows; chain reveal with arrows. | `{ steps[] }` |
| `bar_chart` | Manim `BarChart` with sequential bar reveal. Falls back to a placeholder caption when `data` is too thin. | `{ data: [{label, value}, …] }` |
| `equation_steps` | `MathTex` chain with `TransformMatchingShapes`; falls back to FadeOut/FadeIn on transform failures or LaTeX compile errors. | `{ steps[] }` |
| `neural_net_layer` | Columns of `Circle`s with full-mesh lines; layer-by-layer forward-pass reveal. Truncates to first/last + ellipsis past 7 neurons per layer. | `{ neurons[] }` |

Hardening: every template handles missing / malformed params gracefully — eval errors fall back to a text label, `bar_chart` and `neural_net_layer` render an "insufficient data" placeholder if the input doesn't meet their minimum, `equation_steps` surfaces raw source on LaTeX failure. **No template is allowed to crash the sidecar on bad LLM output.**

Theme constants in `_base.py` (`THEME_BACKGROUND` `#0F172A`, `THEME_ACCENT` `#F59E0B`, `THEME_FOREGROUND` `#FFFFFF`, `THEME_DIM` `#475569`) match the Revideo path's `library` preset and the fast path's `LIBRARY` palette so a chapter mixing all three render paths doesn't visually jolt at segment seams. Phase scheduling helper `phases(run_seconds)` splits each render into 20/60/20 intro/main/outro by default — every template uses it for consistent pacing.

Smoke harness: new `listenai_manim/templates_smoke.py` renders one 4 s MP4 per template into `backend/manim/smoke_output/templates/` with representative inputs. Driven by the new `just manim-templates-smoke` recipe (mirrors the toolchain smoke's env-strip + `LD_PRELOAD=/usr/lib/libfontconfig.so.1` for Arch). Successful run = 8 MP4s for eyeball QA.

Registry exported as `listenai_manim.templates.TEMPLATES: dict[str, Type[TemplateScene]]`. Phase G.5's sidecar reads this dict to dispatch per-request.

#### G.5 — Manim renderer sidecar ✅ (shipped 2026-05-02)

New `listenai_manim/server.py`: long-lived Python process that reads `{version, template_id, params, duration_ms, output_mp4}` JSON per line on stdin, dispatches to `TEMPLATES[template_id]` via the shared `render(...)` helper, emits NDJSON events (`ready` / `started` / `done` / `error` / `bye`) on stdout. Closing stdin = graceful shutdown.

**fd-1 hijack at module load** so subprocesses (Manim shells out to xelatex / dvisvgm / ffmpeg) can't pollute the NDJSON channel:

```py
_REAL_STDOUT_FD = os.dup(1)        # capture real fd 1 for emit()
os.dup2(2, 1)                       # redirect fd 1 to fd 2 (stderr)
sys.stdout = os.fdopen(2, "w", ...) # also redirect Python-level prints
```

Mirrors the Node sidecar's monkey-patched `process.stdout.write` trick from G.5's TS counterpart, with the additional fd-level layer that Python needs because Manim's render path spawns external processes.

**Validation** is split out (`_validate_request`) so the loop body stays readable. Unknown `template_id`, version mismatch, missing `output_mp4`, non-positive duration, and non-`.mp4` extension all surface as one `error` event with a specific message; the sidecar then emits `ready` and waits for the next request without exiting.

**Per-render failures stay non-fatal.** Manim throws? Caught at `_handle_request`, surfaced as `error`, sidecar stays alive. Sidecar threads on a code bug? Caught at the loop level with the type name surfaced. Only stdin EOF triggers `bye` + exit.

**Smoke harness** (`server_smoke.py`): spawns `python -m listenai_manim.server`, pipes two short renders (function_plot + free_body) at it, verifies the event sequence (boot ready → started/done/ready × 2 → eof → bye), checks both MP4s land non-empty. Driven by `just manim-server-smoke` (~30 s) — protocol regression test that doesn't need Rust.

**Console scripts** added to `pyproject.toml`:
- `listenai-manim-server` — invokes the sidecar.
- `listenai-manim-server-smoke` — invokes the smoke harness.

Phase G.6's Rust pool will spawn the sidecar via `LISTENAI_ANIMATE_MANIM_CMD` (configured to `backend/manim/.venv/bin/listenai-manim-server` once that knob lands).

#### G.6 — Per-segment orchestrator ✅ (shipped 2026-05-02)

The piece that finally makes Phase G *do something*. When the book's effective `is_stem` is true **and** `LISTENAI_ANIMATE_FAST_PATH=true` **and** at least one paragraph scene carries a `visual_kind`, the publisher routes to a new per-segment pipeline that mixes Manim diagrams with fast-path prose.

**What landed:**

- **Plumbing** (G.6.a): `Scene::Paragraph` carries optional `visual_kind` + `visual_params`. `ParagraphTile` renamed to `ParagraphInfo` with optional `image_path` + `visual_kind` + `visual_params` (back-compat alias kept). The publisher's `load_paragraph_tiles` reads the new columns from `chapter.paragraphs[]`, emits info rows for any paragraph with at least one of (tile, diagram label) — drops paragraphs that have neither.
- **Manim sidecar pool** (G.6.b): new `backend/api/src/animation/manim_sidecar.rs`, structurally identical to the Revideo `RendererPool` (semaphore-gated slots, lifecycle bounds, NDJSON IPC). Different request shape (`{version, template_id, params, duration_ms, output_mp4}`) and event mapping. Spawns a managed Python interpreter pointed at `LISTENAI_ANIMATE_MANIM_CMD`; honors `LISTENAI_ANIMATE_MANIM_LD_PRELOAD` for the Arch libfontconfig workaround.
- **Per-segment renderer** (G.6.c): new `backend/api/src/animation/segments.rs`. `render_chapter(spec, ffmpeg, hwenc, vaapi_device, manim_pool, progress)` walks scenes, routes diagrams → Manim and the rest → fast-path single-scene mini-renders (with a tempfile silent-WAV input so `fast_path::render`'s `-map 1:a` is satisfied). ffmpeg-concats video-only segments, then ffmpeg-muxes the chapter WAV. Scratch dir kept on failure, nuked on success. `has_diagram_scenes(spec)` predicate on the same module.
- **Handler routing** (G.6.d): `AnimateChapterHandler` gains a lazy `manim_pool` `OnceCell<Option<Arc<...>>>` (None when `animate_manim_cmd` is empty — segments still take the path, but diagrams fall back to prose with a warn log). `load_is_stem(state, audiobook_id)` reads `stem_override` + `stem_detected` and applies the same fallback the detail endpoint uses. Three-way branch in `run()`: mock → existing mock; segments-eligible → `segments::render_chapter`; fast-path → existing `fast_path::render`; else → Revideo pool.
- **Cache awareness** (G.6.e): new `cache::FFMPEG_STEM_PATH_LABEL = "ffmpeg-stem-v1"`. Folded into the spec hash, so toggling STEM (or the override) invalidates the cache cleanly; you don't reuse a prose MP4 when the user just enabled diagrams.
- **Config + env**: `LISTENAI_ANIMATE_MANIM_CMD`, `LISTENAI_ANIMATE_MANIM_PYTHON_BIN`, `LISTENAI_ANIMATE_MANIM_LD_PRELOAD` — documented in `.env.example` with the Arch-specific libfontconfig preload spelled out.

**What's deferred:**

- Concurrency tuning of the Manim pool (matches `animate_concurrency` for now; if Manim's startup cost is too tax-heavy for parallel chapter renders we may want to cap it differently).
- Progress events from the Manim sidecar: it emits `started` / `done` only today; finer-grained per-frame progress would need polling Manim's tqdm output (out of scope for v1).
- Revideo-path support for diagrams (intentionally not wired — Manim diagrams require fast-path mode).

**Tests:** 102 workspace tests green (was 97; +3 manim_sidecar + 2 segments). `cargo check` clean; `cargo clippy` clean on every G.6 file.

#### G.7 — Frontend signal

Small "📐 N diagrams" badge on `AnimationRow` when the chapter has at least one diagram scene. Cheap UX win.

## 5. Operational Concerns

- **Toolchain:** the render host now needs Node 20 + ffmpeg (already required) + system fonts. Add to the Dockerfile in a single layer; document under `docs/animation/render-host.md`.
- **Disk:** 1080p H.264 at CRF 22 ≈ 8 MB/min. A 60-min book = ≈ 0.5 GB on disk per language. Add a Phase-F job to garbage-collect `ch-N.video.mp4` once a publication's `youtube_url` is set and older than 30 days. Reuse the existing `JobKind::Gc` flow.
- **Determinism:** Revideo is pixel-deterministic given the same inputs and version. Pin the Revideo version in `backend/render/package.json` and check the lockfile in. Renders should be re-runnable for QA.
- **Offline / mock mode:** add `ANIMATE_MOCK=true` that returns a 5-second black-with-title MP4, mirroring how `MockTts` works today. This keeps `cargo test` and CI fast.
- **Licensing trail:** add a top-level `NOTICES.md` entry for Revideo (MIT) and FFmpeg (LGPL build) — a SaaS audit will ask.

## 6. Risks & Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Revideo OSS cadence slows further (team focused on Midrender) | medium | Pin version + vendor a working tag. Contract is `JSON in, MP4 out` — swap to Remotion later if needed. |
| Render-time blows the job worker budget | medium | Per-chapter parallelism + WebCodecs is fast (~0.3× realtime expected); benchmark in Phase A and gate Phase D on the result. |
| Node + Chrome footprint on render host | low (Revideo path uses WebCodecs, not Chromium) | If we end up on Remotion later, accept the extra 400 MB image layer; it's well-trodden. |
| YouTube rejects long single-image-replaced-by-video uploads for resolution mismatch | low | Always render at exactly `1920×1080@30`; existing upload flow already handles this size. |
| User sees jarring quality cliff between animated and non-animated chapters | medium | Animation is **all-or-nothing per book** — the publish flow rejects mixed states. |
| Whisper word-timing in Phase F adds latency | low | Phase F is gated; Phase A–E ship without it. |

## 7. Out of Scope (v1)

- 3D scenes, character animation, lip-sync mouths.
- Per-user style training / custom themes beyond the three presets.
- iOS-side rendering (animation is server-rendered only; iOS plays the resulting MP4 like any other video).
- Real-time preview in the browser (Phase E preview is the rendered MP4, not a live Revideo player).
- Localization-aware typography beyond the languages we already render audio for.

## 8. Branch & Commit Convention

- Branch: `feature/animation`, off `main`.
- Phased commits, each compiling and passing `just check`:
  - `feat(animate): phase A — render sidecar + JobKind::AnimateChapter`
  - `feat(animate): phase B — scene planner`
  - `feat(animate): phase C — scene library + themes`
  - `feat(animate): phase D — youtube publisher integration`
  - `feat(animate): phase E — frontend`
  - `feat(animate): phase F — polish (word timing, quota, GC)` *(deferrable to a follow-up branch)*
- PR back to `main` after Phase E. Phase F is a follow-up PR.

## 9. Open Questions for the User

1. **Remotion swap-in path?** Are we comfortable shipping on Revideo MIT for v1 and treating Remotion as a paid-tier upgrade later, or do you want the Remotion Company License factored into v1 from day one?
2. **Visual direction?** Three presets (`library`, `parchment`, `minimal`) — does that match the brand, or should one of them be replaced by something more illustrative (e.g., AI-generated background art per scene)?
3. **Whisper for word timing in v1 or v2?** Constant-cadence karaoke is "fine" but visibly drifts; real word timing is a quality jump. Cost is one Whisper pass per chapter, cached.
4. **Per-paragraph illustrations:** are `paragraph-tiles/p{n}.webp` already being generated end-to-end today, or is that another upcoming feature we should treat as optional input?
