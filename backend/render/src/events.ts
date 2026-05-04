// NDJSON progress events emitted on stdout. The Rust publisher
// (`backend/api/src/jobs/publishers/animate.rs`) parses one of these
// per line and forwards them to the WebSocket progress hub.
//
// Rules:
//   * Exactly one JSON object per line, terminated with `\n`.
//   * Only `emit()` may write to the real stdout — see the redirect
//     below. `console.log` is automatically rerouted to stderr so
//     stray third-party chatter (Revideo's puppeteer wrapper logs
//     `Worker 0: JSHandle@object` for every Chromium console event)
//     can't pollute the NDJSON stream.
//   * Anything else (warnings, traces) goes to stderr.

// Capture the real stdout writer BEFORE any third-party module
// overrides it. We use this exclusively for NDJSON events; everything
// else gets rerouted to stderr below.
const ORIGINAL_STDOUT_WRITE = process.stdout.write.bind(process.stdout);

// Reroute every other `process.stdout.write` call to stderr. This
// catches Revideo's `console.log("Worker 0: …")` chatter and any
// other library that thinks it's free to write to stdout. The Rust
// pool reads stdout strictly as NDJSON; without this redirect the
// `next_event` parser fires `non-NDJSON line` debug logs once per
// puppeteer page event, which is hundreds of lines per render.
process.stdout.write = ((
  chunk: Uint8Array | string,
  ...args: unknown[]
) =>
  (process.stderr.write as (...a: unknown[]) => boolean).call(
    process.stderr,
    chunk,
    ...args,
  )) as typeof process.stdout.write;

export type RenderEvent =
  | { type: 'started' }
  | { type: 'frame'; frame: number; total: number }
  | { type: 'encoding'; pct: number }
  | { type: 'done'; mp4: string; duration_ms: number }
  | { type: 'error'; message: string }
  // Long-lived sidecar (server.ts) only: emitted once at boot and again
  // between renders to signal that the next spec can be sent. Never
  // emitted by the one-shot `cli.ts`.
  | { type: 'ready' }
  // Long-lived sidecar only: emitted just before exit when stdin
  // closes, so the Rust pool can distinguish a clean shutdown from a
  // crash.
  | { type: 'bye' };

export function emit(evt: RenderEvent): void {
  // Always go via the captured stdout reference — the property has
  // been monkey-patched to redirect to stderr, so `process.stdout.
  // write(...)` from anywhere else (including this file once the
  // module has loaded) would land on the wrong stream.
  ORIGINAL_STDOUT_WRITE(JSON.stringify(evt) + '\n');
}
