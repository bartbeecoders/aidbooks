# mold-service

A standalone HTTP wrapper around [utensils/mold](https://github.com/utensils/mold)'s
`mold serve`. Owns the AidBooks-flavored policy that used to live in
`backend/api/src/llm/mold.rs`:

- GPU semaphore (single-model-at-a-time by default).
- OOM cooldown — after a CUDA OOM, hold the in-process semaphore for
  ~65s so the worker outlasts mold's 3-strike degrade window.
- Default model (`flux2-klein:q8`), default step count per model
  family, default CFG guidance per family.
- 9:16 shorts vs square portrait dimensions.

The backend talks to this service via HTTP instead of going straight to
`mold serve`, so the API only needs to know `prompt`, `model`, and
whether it's a short.

## Architecture

```
+---------+        +-----------------+        +-------------+
| backend |  HTTP  |  mold-service   |  HTTP  | mold serve  |
|  /api   | -----> |   (axum, this   | -----> |  (upstream) |
|         |        |     crate)      |        |             |
+---------+        +-----------------+        +-------------+
                   ^  policy
                   |  semaphore
                   |  OOM cooldown
                   |  base64 encoding
```

## Endpoints

| Method | Path                  | Notes                                       |
|--------|-----------------------|---------------------------------------------|
| GET    | `/healthz`            | Service + upstream liveness. Unauth.        |
| GET    | `/v1/defaults`        | Preview policy defaults. Query: `model`, `is_short`. |
| POST   | `/v1/generate`        | Generate an image. Returns base64.          |
| POST   | `/v1/models/pull`     | Pull a model on the upstream.               |
| DELETE | `/v1/models/unload`   | Drop every loaded model from VRAM.          |

### `POST /v1/generate`

```jsonc
{
  "prompt": "a cat",            // required
  "model": "flux2-klein:q8",    // optional, defaults to DEFAULT_MOLD_MODEL
  "is_short": false,            // sets default width/height when not given
  "width": 1024,                // optional, must be a multiple of 16
  "height": 1024,               // optional, must be a multiple of 16
  "steps": 4,                   // optional, defaults per model family
  "guidance": 0.0,              // optional, defaults per model family
  "seed": 42,                   // optional
  "negative_prompt": "...",     // optional
  "output_format": "png"        // png | jpeg | webp
}
```

Response:

```jsonc
{
  "image_base64": "...",        // raw image, base64-encoded
  "content_type": "image/png",
  "width": 1024,
  "height": 1024,
  "model": "flux2-klein:q8",
  "steps": 4,
  "guidance": 0.0,
  "seed_used": 12345,           // from the `x-mold-seed-used` header
  "output_format": "png"
}
```

## Configuration

All env-driven. Defaults make `cargo run` work against a loopback
`mold serve` on port 7680.

| Env var                   | Default                     | Meaning                                   |
|---------------------------|-----------------------------|-------------------------------------------|
| `MOLD_SERVICE_BIND`       | `127.0.0.1`                 | Bind address.                             |
| `MOLD_SERVICE_PORT`       | `7681`                      | Bind port.                                |
| `MOLD_SERVICE_API_KEY`    | _(unset)_                   | When set, every non-`/healthz` call must include `X-Api-Key`. |
| `MOLD_UPSTREAM_URL`       | `http://127.0.0.1:7680`     | Upstream `mold serve` URL.                |
| `MOLD_UPSTREAM_API_KEY`   | _(unset)_                   | Forwarded to mold as `X-Api-Key`.         |
| `MOLD_MAX_CONCURRENCY`    | `1`                         | In-flight `/v1/generate` cap.             |
| `MOLD_TIMEOUT_SECS`       | `300`                       | Per-generate HTTP timeout.                |
| `MOLD_PULL_TIMEOUT_SECS`  | `3600`                      | Per-pull HTTP timeout.                    |
| `MOLD_OOM_COOLDOWN_SECS`  | `65`                        | Semaphore hold after a detected OOM.      |

## Run

```bash
# inside this folder
cargo run --release

# with explicit config
MOLD_UPSTREAM_URL=http://192.168.1.10:7680 \
MOLD_SERVICE_PORT=7681 \
MOLD_SERVICE_API_KEY=hunter2 \
    cargo run --release
```

`mold serve` itself needs to be running separately (see the project
root's `mold/` for the upstream code, or `scripts/dev.sh` which already
manages it).

## Tests

### Rust integration tests

Spin up the real axum router against an in-process upstream stub. No GPU
needed — these run in CI.

```bash
cargo test
```

### Bash smoke tests

End-to-end against a real running mold serve. The runner boots
`mold-service` for you and tears it down on exit.

```bash
# all of health + generate, against a real GPU
./scripts/test.sh

# health only (no GPU needed if you point at the service alone)
MOLD_SKIP_GENERATE=1 ./scripts/test.sh

# generate a portrait short with a custom prompt
MOLD_TEST_IS_SHORT=true \
MOLD_TEST_PROMPT="a wide cinematic painting of a fox in the snow" \
    ./scripts/test.sh

# reuse an already-running mold-service
MOLD_SKIP_BOOT=1 MOLD_SERVICE_URL=http://127.0.0.1:7681 \
    ./scripts/test.sh

# also exercise pull/unload (slow!)
MOLD_TEST_PULL=1 MOLD_TEST_UNLOAD=1 ./scripts/test.sh
```

Individual scripts can be run on their own — they each respect
`MOLD_SERVICE_URL` and `MOLD_SERVICE_API_KEY`:

```bash
./scripts/health.sh
./scripts/generate.sh
./scripts/pull.sh         # model from $MOLD_TEST_MODEL
./scripts/unload.sh
```

## Migrating the backend

The backend's `llm` rows with `provider = "mold"` point at the
mold-server via `base_url`. After deploying mold-service, update those
rows to point at the mold-service URL (typically `http://127.0.0.1:7681`)
— the backend's mold client now speaks the mold-service protocol, not
the raw `mold serve` protocol.
