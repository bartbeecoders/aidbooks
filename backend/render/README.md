# @listenai/render — animation sidecar

Node + Revideo sidecar that turns a chapter `SceneSpec` (JSON) into an MP4. Driven by the Rust
`animate` job publisher (`backend/api/src/jobs/publishers/animate.rs`).

## Contract

```text
$ node dist/cli.js < scene.json > progress.ndjson 2> renderer.log
```

- **stdin**: one JSON object matching `backend/api/src/animation/spec.rs::SceneSpec`. Strict version
  match (`SCENE_SPEC_VERSION` / `EXPECTED_VERSION`).
- **stdout**: NDJSON, one event per line. See `src/events.ts` for the union.
- **stderr**: free-form logs. The Rust caller buffers the tail and folds it into the failure message
  on a non-zero exit.
- **exit code**: `0` = success, non-zero = failure.

## Phase status

This is the **Phase A** placeholder. The CLI validates the spec, opens Revideo's `renderVideo`, and
plays the chapter audio against the cover image with a "ListenAI — Animation Preview" overlay. Real
per-paragraph karaoke text, waveform-reactive accents, and the `library` / `parchment` / `minimal`
theme library land in Phase C.

## Asset paths

Revideo runs scenes inside a Chromium-backed Vite server. We rewrite the absolute host paths from
the spec (`audio.wav`, `background.src`) to `file://` URLs in `cli.ts::pathToUrl`. If your
deployment can't serve `file://` from the renderer's Chromium, point
`LISTENAI_ANIMATE_RENDERER_CMD` at a wrapper that copies inputs into `public/` first.

## Local dev

```sh
cd backend/render
npm install
npm run build              # → dist/cli.js
echo '{...spec...}' | npm run cli
```

For a fast iteration loop without the build step: `npm run dev` (uses `tsx`).

## Mock mode

The Rust side has `LISTENAI_ANIMATE_MOCK=true` that bypasses this sidecar entirely and writes a
black 1080p MP4 via ffmpeg. CI uses that — keep this renderer working for real renders only.
