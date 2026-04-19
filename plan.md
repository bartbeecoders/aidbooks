# ListenAI — Implementation Plan

> AI-powered audiobook generator. Web (React + Vite + TS) and iOS client, Rust backend, SurrealDB (embedded), OpenRouter for content generation, x.ai Realtime API for voice synthesis.

This plan is organised into **10 phases**. Each phase contains **Goals**, **Steps**, **Deliverables**, and a **Done when** checklist. Phases are roughly sequential, but Phase 6 (Web Frontend) can run in parallel with Phases 3–5 once Phase 2 exposes stable API contracts.

---

## Guiding Principles

1. **Vertical slices first.** Ship one end-to-end flow (text-in → audio-out → playable in library) before polishing edges.
2. **Typed contracts everywhere.** Share OpenAPI schema between Rust and TS to eliminate drift.
3. **Everything is a job.** Content + voice generation are long-running; model them as durable jobs from day one (don't bolt this on later).
4. **Fail gracefully, resume cheaply.** A chapter failing at minute 47 of a 60-minute book should not discard the first 46 minutes.
5. **Admin-configurable, not hard-coded.** LLM list, voice list, prompt templates, quotas, prices — all stored in DB, editable at runtime.
6. **Cost-aware.** Each job logs token usage + TTS seconds so pricing and quotas can be tuned from real data.

---

## High-Level Architecture

```
┌──────────────┐     ┌──────────────┐      ┌────────────────┐
│  Web (Vite)  │     │  iOS Swift   │      │  Admin Panel   │
│  React + TS  │     │    UIKit     │      │  (same web)    │
└──────┬───────┘     └──────┬───────┘      └────────┬───────┘
       │                    │                       │
       └────────────────────┴───────────────────────┘
                            │ HTTPS + WebSocket (progress)
                            ▼
                 ┌──────────────────────┐
                 │   Rust Backend       │
                 │   Axum + Tower       │
                 │   ├─ REST API        │
                 │   ├─ Job runner      │
                 │   └─ WS progress hub │
                 └──┬───────────────┬───┘
                    │               │
          ┌─────────▼────┐   ┌──────▼────────────┐
          │  SurrealDB    │   │  Object store    │
          │  (embedded,   │   │  (local FS,      │
          │  RocksDB)     │   │  S3-compat soon) │
          └──────┬────────┘   └──────────────────┘
                 │
       ┌─────────┼─────────┐
       ▼         ▼         ▼
  ┌────────┐ ┌────────┐ ┌──────────┐
  │OpenRou-│ │ x.ai   │ │ flux2    │
  │ter LLM │ │ Voice  │ │ (covers) │
  └────────┘ └────────┘ └──────────┘
```

---

## Status Tracker

| Phase | State | Commit |
|-------|-------|--------|
| 0 — Scaffolding                  | ✅ Complete | `f5371b0` |
| 1 — Backend Foundation           | ✅ Complete | `f16b66f` |
| 2 — Authentication & Users       | ✅ Complete | `6da4d49` |
| 3 — Content Generation           | ✅ Complete | (current branch) |
| 4 — Voice Synthesis              | ⏳ Next     | — |
| 5 — Job Orchestration            | ⏳          | — |
| 6 — Web Frontend                 | ⏳          | — |
| 7 — Admin Panel                  | ⏳          | — |
| 8 — iOS App                      | ⏳          | — |
| 9 — Monetisation                 | ⏳          | — |
| 10 — Polish & Launch             | ⏳          | — |

---

## Phase 0 — Project Scaffolding & Tooling

**Goal:** empty repo → reproducible dev environment, CI, formatters, commit hooks.

### Steps
1. Initialise monorepo layout:
   ```
   /backend         Rust (cargo workspace: api, core, jobs, db)
   /frontend        React + Vite + TS
   /ios             Xcode project (added in Phase 8)
   /shared          OpenAPI spec + generated TS types
   /storage         Git-ignored runtime audio + db files
   /docs            Architecture notes, ADRs
   /Vibecoding      (existing) — project brief lives here
   ```
2. Add `README.md`, `.editorconfig`, `.gitignore`, `LICENSE`.
3. Rust workspace with `backend/Cargo.toml` members: `api`, `core`, `jobs`, `db`. Pin MSRV in `rust-toolchain.toml`.
4. Frontend: `npm create vite@latest frontend -- --template react-ts`, add Tailwind + shadcn/ui + Radix + TanStack Router + Zustand + wavesurfer.js.
5. Add `.env.example` with every required key (`OPENROUTER_API_KEY`, `XAI_API_KEY`, `JWT_SECRET`, `DATABASE_PATH`, `STORAGE_PATH`, `SMTP_*`, `STRIPE_*`).
6. **CI (GitHub Actions):** `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `npm run lint`, `npm run typecheck`, `npm test`.
7. **Pre-commit hooks** via `lefthook` or `pre-commit`: rustfmt, eslint, prettier, commit-msg lint (conventional commits).
8. **Dev harness:** `just dev` (or `make dev`) spins backend + frontend concurrently; `just e2e` runs Playwright suite.

### Done when
- `cargo run -p api` serves `/health` returning `{ "status": "ok" }`.
- `npm run dev` shows a blank React app wired to the backend health probe.
- CI green on a trivial PR.

---

## Phase 1 — Backend Foundation ✅

**Goal:** Axum server with SurrealDB embedded, observability, error model, config.

> **Done.** The server boots, opens embedded SurrealDB at `./storage/db`, runs forward-only migrations, seeds five x.ai voices and two OpenRouter LLM configs, and serves `/health`, `/ready`, `/openapi.json`. JSON log lines carry the `request_id` in the span fields, and the `x-request-id` header round-trips on requests and responses.
>
> **What shipped that wasn't in the original plan:**
> - `UPSERT`-based idempotent seed in Rust instead of a second SQL migration — lets us iterate on seed data without writing new migration files.
> - `Config.toml` support via `figment` alongside env vars.
> - JSON `/openapi.json` endpoint (utoipa), usable today with Swagger UI / Redocly / codegen.
> - `/ready` swallows DB errors into `reachable: false` (readiness probes must never 5xx).
>
> **Deferred to a later phase:**
> - Admin user seed — deferred to Phase 2 where password hashing lands.
> - OpenTelemetry exporter — not needed until Phase 10.


### Steps
1. **Crate: `core`** — domain types (`User`, `Audiobook`, `Chapter`, `Voice`, `Llm`, `Job`, `JobStatus`). Use `serde` + `utoipa` for OpenAPI schema derivation.
2. **Crate: `db`** — thin wrapper around SurrealDB. Embedded with RocksDB backend:
   ```toml
   surrealdb = { version = "2", features = ["allocator", "storage-rocksdb"] }
   ```
   Configure `tokio::runtime::Builder` with multi-thread, stack size 10 MiB, and the `mimalloc` allocator (per SurrealDB perf guide).
3. **Crate: `api`** — Axum router with middleware stack:
   - `TraceLayer` (tower-http) for structured logs
   - `CorsLayer`
   - `RequestIdLayer`
   - `TimeoutLayer`
   - `CompressionLayer`
   - Custom `auth` layer (added in Phase 2)
4. **Error model:** `thiserror` enum → `IntoResponse` that maps to `{ code, message, request_id }` with correct HTTP status. Never leak internal errors.
5. **Config:** `figment` or `config-rs` layered: defaults → `config.toml` → `.env` → env vars.
6. **Observability:** `tracing` + `tracing-subscriber` with JSON output in prod, pretty in dev. Optional OpenTelemetry exporter behind feature flag.
7. **Migrations:** `core/migrations/` with timestamped `.surql` files; on startup, run pending migrations inside a transaction and record them in a `_migrations` table.
8. **Seed script:** creates a default admin user, a handful of LLMs, and voices lifted from x.ai's catalogue (Eve, Ara, Rex, Sal, Leo).

### Done when
- Server boots, creates DB file under `/storage/db`, runs migrations, and exposes OpenAPI JSON at `/openapi.json` (reachable from the frontend via the dev proxy at `/api/openapi.json`).
- Logs show structured JSON with request id propagation.

### Verified

```text
cargo run -p api  →  http://127.0.0.1:8787/health   { "status": "ok", ... }
                     http://127.0.0.1:8787/ready    { "status":"ready","db":{"reachable":true,...} }
                     http://127.0.0.1:8787/openapi.json  (OpenAPI 3.1 doc)

JSON log line example (every line within a request carries request_id):
{ "timestamp":"...","level":"DEBUG","fields":{"message":"finished processing request","latency":"0 ms","status":200},
  "span":{"method":"GET","request_id":"probe-xyz-789","route":"/ready","uri":"/ready","name":"http"} }
```

### Key files

| File | Purpose |
|------|---------|
| `backend/Cargo.toml`                          | Workspace deps: surrealdb (kv-rocksdb), utoipa, figment, mimalloc |
| `backend/core/src/domain/`                    | `User`, `Audiobook`, `Chapter`, `Voice`, `Llm`, `Job` + enums |
| `backend/core/src/{id,error,config}.rs`       | `UserId`/`AudiobookId`/... newtypes, `Error`, `Config` via figment |
| `backend/db/src/lib.rs`                       | Embedded SurrealDB (RocksDB) handle |
| `backend/db/migrations/0001_init.surql`       | Forward-only schema (tables, fields, indexes) |
| `backend/db/src/migrate.rs`                   | `_migrations`-tracked runner |
| `backend/db/src/seed.rs`                      | Idempotent UPSERT of 5 voices + 2 LLMs |
| `backend/api/src/{main,app,state,error}.rs`   | Custom Tokio runtime (10 MiB stack, mimalloc), middleware stack, `ApiError` → `IntoResponse` |
| `backend/api/src/openapi.rs`                  | `#[derive(OpenApi)]` root doc served at `/openapi.json` |

---

## Phase 2 — Authentication & Users ✅

**Goal:** account lifecycle, JWT auth, RBAC (user vs admin), quotas.

> **Done.** Argon2id password hashing (with a server-wide pepper as Argon2's secret parameter) + HS256 access-token JWTs + rotating opaque refresh tokens with reuse detection. Six endpoints (`register`, `login`, `refresh`, `logout`, `GET /me`, `PATCH /me`), bearer-auth security scheme in OpenAPI, and a dev-only demo admin seeded at `demo@listenai.local` / `demo` when `LISTENAI_DEV_SEED=true`.
>
> **What shipped beyond the plan's explicit wording:**
> - Pepper is applied via Argon2's secret parameter (not naive concat) so a leaked `password_hash` column without the pepper is un-brute-forceable.
> - Refresh tokens are stored as HMAC-SHA256 of the raw token, keyed with the pepper — plaintext never touches the DB.
> - Rotation-on-every-refresh with reuse detection: presenting a revoked refresh token revokes all of that user's other sessions (classic session-hijack mitigation).
> - `/openapi.json` includes a `bearer` security scheme so Swagger UI / Redocly gives a "Try it" login UX.
>
> **Deferred (docs updated):**
> - Social login (Google/Apple OAuth) — Phase 8 (needs the frontend + mobile clients first).
> - `forgot`, `reset`, `verify-email` — wait for SMTP setup.
> - Tier / quota enforcement — will land alongside Phase 3 where it actually gates something.
> - Audit log — Phase 10 hardening.
> - Login rate-limiting — Phase 10.


### Steps
1. **Data model:**
   ```
   user { id, email, password_hash, display_name, role, tier, created_at,
          email_verified_at, stripe_customer_id?, quota_overrides? }
   session { id, user_id, refresh_token_hash, user_agent, ip, expires_at }
   ```
2. **Password hashing:** `argon2` with per-install pepper in env. Rate-limit login (`tower-governor`).
3. **JWT:** short-lived access token (15 min, HS256) + long-lived refresh token (30 d, rotated on use, stored hashed in `session`).
4. **Endpoints:** `POST /auth/register`, `/auth/login`, `/auth/refresh`, `/auth/logout`, `GET /me`, `PATCH /me`, `POST /auth/forgot`, `POST /auth/reset`, `POST /auth/verify-email`.
5. **Social login:** OAuth (Google, Apple) via `oauth2` crate. Store provider → user id mapping in `identity` table.
6. **Role-based guard:** `RequireRole(Admin)` extractor for admin routes.
7. **Tiers & quotas:** `tier` table with `name`, `monthly_audiobook_seconds`, `max_length_minutes`, `max_voices_per_book`, `allowed_llms: [id]`. Checked in generation pipeline (Phase 5).
8. **Audit log:** append-only `audit_event` table for security-relevant actions (login, password reset, role change, quota override).

### Done when
- A new user can register, log in, and call `/me`.
- Refresh rotation blacklists the old token; reuse of a rotated refresh token logs the user out of all sessions (reuse-attack detection).
- (Admin list/change endpoints are deferred to Phase 7 — the `RequireAdmin` extractor already exists, stubbed, for that phase to wire up.)

### Verified (clean boot)

| Flow | Expected | Got |
|------|----------|-----|
| register alice@example.com / 16-char password | 200 + token pair | ✅ |
| register alice@example.com again | 409 conflict | ✅ |
| register with 4-char password | 400 validation | ✅ |
| register with malformed email | 400 validation | ✅ |
| login demo@listenai.local / demo | 200 + token pair | ✅ |
| login with wrong password | 401 | ✅ |
| login with unknown email | 401 | ✅ |
| GET /me without Bearer | 401 | ✅ |
| GET /me with Bearer | 200 + user | ✅ |
| PATCH /me display_name | 200 + updated user | ✅ |
| refresh rotation | 200 + different access + refresh | ✅ |
| reuse old refresh after rotation | 401 + all sessions revoked | ✅ |
| next refresh after reuse alarm | 401 | ✅ |
| logout with access + refresh | 204, session revoked | ✅ |

### Key files

| File | Purpose |
|------|---------|
| `backend/core/src/crypto.rs`                 | argon2 hash/verify, HMAC-SHA256 token hashing, CT equality |
| `backend/core/src/config.rs`                 | `jwt_secret`, `password_pepper`, `dev_seed`, TTL knobs |
| `backend/db/migrations/0002_session.surql`   | `session` table schema |
| `backend/db/src/seed.rs`                     | dev-only demo admin upsert when `dev_seed=true` |
| `backend/api/src/auth/claims.rs`             | `AccessClaims`, `AuthedUser` |
| `backend/api/src/auth/tokens.rs`             | `issue_access_token`, `verify_access_token` |
| `backend/api/src/auth/extractor.rs`          | `Authenticated`, `RequireAdmin` (stub for Phase 7) |
| `backend/api/src/handlers/auth.rs`           | `register`, `login`, `refresh`, `logout` |
| `backend/api/src/handlers/me.rs`             | `GET /me`, `PATCH /me` |
| `backend/api/src/openapi.rs`                 | Bearer security scheme + all auth schemas |

### Demo credentials (DEV ONLY)

```
email:    demo@listenai.local
password: demo
role:     admin
tier:     pro
```

Active only when `LISTENAI_DEV_SEED=true`. A loud `WARN` log is emitted on every startup when dev seed is on.

---

## Phase 3 — Content Generation (OpenRouter) ✅

**Goal:** topic → structured audiobook outline → chapter prose, persisted and editable before narration.

> **Done.** A typed OpenRouter client (reqwest, rustls-tls) with a built-in **mock mode** when `OPENROUTER_API_KEY` is empty, DB-backed prompt templates (seeded from markdown files at build time via `include_str!`), synchronous outline generation on `POST /audiobook`, async chapter generation via `tokio::spawn` that transitions `outline_ready → chapters_running → text_ready`, per-chapter edit/regenerate, random-topic generator, and a per-call cost log in `generation_event`.
>
> **What shipped that wasn't spelled out in the original plan:**
> - **Mock mode** on the LLM client so devs (and CI) can run the whole content pipeline end-to-end without a real key. Loud `WARN` log on boot whenever it's active.
> - **`.surql` status widening via `DEFINE FIELD OVERWRITE`** — forward-only migration pattern so the schema constraint stays in sync with the Rust enum without a DB wipe.
> - **Markdown-file prompts** (`backend/db/src/prompts/*.md`) embedded via `include_str!` and upserted into `prompt_template` on boot — future versions bump `version` and stay loadable via the `ORDER BY version DESC` lookup.
> - **Ownership enforcement**: `GET|PATCH|DELETE /audiobook/:id` return `404` (not `403`) to any non-owner — never leaks existence.
> - **Tiny `{{var}}` renderer** (not Tera/Handlebars) so single-brace JSON examples inside the prompt body pass through untouched.
>
> **Deferred:**
> - Durable jobs + WebSocket progress → Phase 5. Current chapter generation is `tokio::spawn` fire-and-forget with progress visible through `audiobook.status` polling.
> - Per-tier quota enforcement → when Phase 9 billing lands.
> - Admin CRUD on `llm` / `prompt_template` / `voice` → Phase 7.
> - Streaming SSE from the LLM back to the client → Phase 5/6.
> - Speaker-tagged multi-voice chapters → Phase 4.
> - Safety moderation pass → Phase 10.


### Steps
1. **Pluggable LLM registry:** DB-backed `llm` table `{ id, name, provider: "openrouter", model_id, context_window, cost_per_1k_prompt, cost_per_1k_completion, enabled, default_for: [outline|prose|title] }`. Admin-editable.
2. **Prompt templates table:** `prompt_template { id, role: "outline|chapter|title|cover|random_topic", body_md, variables: [..], version, active }`. Versioned so history is preserved. Editable by admin.
3. **OpenRouter client:** use the `openrouter_api` crate for typed requests, streaming SSE, and retry-with-backoff. Wrap it in a small trait `LlmClient` so we can swap providers.
4. **Outline step:**
   - Input: `{ topic, length: short|medium|long, genre, voice_style }`.
   - Length maps to chapter count + target words per chapter (e.g. short: 3 ch × 500 w; medium: 6 × 1200 w; long: 12 × 2500 w — configurable per tier).
   - Output JSON: `{ title, subtitle, chapters: [{ number, title, synopsis, target_words }] }`. Validated with `jsonschema`; if invalid, retry with repair prompt up to N times.
5. **Chapter generation step:**
   - One LLM call per chapter, streaming. Passes outline + previous chapter's ending (last ~400 tokens) for continuity. Stored as markdown with speaker tags (`> [narrator]`, `> [character:Maya]`) so Phase 4 can assign voices.
6. **Random topic generator:** dedicated endpoint that prompts the LLM for a creative topic + auto-selects genre/length. Optionally seeded by user-selected themes ("sci-fi", "history of X", "for kids").
7. **Editable drafts:** outline and chapters are editable via `PATCH /audiobook/:id/outline` and `PATCH /audiobook/:id/chapter/:n` before narration starts. A "regenerate chapter" action re-runs the LLM with updated context.
8. **Cost tracking:** every call records `prompt_tokens`, `completion_tokens`, `llm_id`, `cost_usd` on a `generation_event` row, linked to the audiobook and user for quota accounting.
9. **Safety:** run outline + chapter text through a moderation pass (OpenAI moderation endpoint or a dedicated OpenRouter model) and store flags. Block generation if hard-fail categories trigger.

### Done when
- `POST /audiobook` with a topic returns an `audiobook` row in `status=outline_ready` with a valid chapter list.
- `POST /audiobook/:id/generate-chapters` (now `202 Accepted` + background task; WS progress is Phase 5) leaves the audiobook in `status=text_ready`.
- (Admin OpenRouter-model CRUD is deferred to Phase 7; LLM rows are DB-editable via direct SurrealQL today.)

### Verified (clean boot, mock LLM)

| Flow | Expected | Got |
|------|----------|-----|
| `POST /topics/random` | 200 + `{ topic, genre, length }` | ✅ |
| `POST /audiobook` (sync outline) | 200 + `status: outline_ready` + N chapters | ✅ |
| `POST /audiobook/:id/generate-chapters` | 202, later polls show `chapters_running → text_ready` | ✅ (2 s in mock mode) |
| `GET /audiobook` / `GET /audiobook/:id` | owner-scoped list + detail | ✅ |
| `PATCH /audiobook/:id` | title edited | ✅ |
| `PATCH /audiobook/:id/chapter/:n` | title + synopsis edited | ✅ |
| `POST /audiobook/:id/chapter/:n/regenerate` | chapter body rewritten, `status: text_ready` | ✅ |
| `DELETE /audiobook/:id` | 204, chapters also gone | ✅ |
| other-user GET / DELETE on owned book | 404 (never 403 — existence not leaked) | ✅ |
| `GET /voices` / `GET /llms` | enabled rows, JSON | ✅ |
| 1-char topic | 400 validation | ✅ |

### Key files

| File | Purpose |
|------|---------|
| `backend/core/src/domain/{prompt,generation_event}.rs` | New domain types |
| `backend/core/src/domain/audiobook.rs`                 | Expanded `AudiobookStatus` + length helpers (`chapter_count`, `words_per_chapter`) |
| `backend/db/migrations/0003_content.surql`             | `prompt_template`, `generation_event`, `OVERWRITE` status constraints |
| `backend/db/src/prompts/{outline,chapter,random_topic}_v1.md` | Seeded prompt bodies |
| `backend/db/src/seed.rs`                               | Upserts prompts every boot |
| `backend/api/src/llm/openrouter.rs`                    | reqwest client + mock mode |
| `backend/api/src/generation/prompts.rs`                | `{{var}}` rendering |
| `backend/api/src/generation/outline.rs`                | Outline generation + cost log |
| `backend/api/src/generation/chapter.rs`                | Per-chapter generation with previous-chapter ending carry-over |
| `backend/api/src/handlers/audiobook.rs`                | Full audiobook CRUD + generate/regenerate |
| `backend/api/src/handlers/topics.rs`                   | `POST /topics/random` |
| `backend/api/src/handlers/catalog.rs`                  | `GET /voices`, `GET /llms` |

### Test coverage added

- `generation::prompts` (3 tests) — variable interpolation, unknown-marker fallthrough, JSON-brace safety.
- `llm::openrouter` (2 tests) — mock outline returns valid JSON with correct chapter count; mock chapter returns plain prose.


---

## Phase 4 — Voice Synthesis (x.ai Realtime)

**Goal:** transform `text_ready` audiobook into an M4B audio file with per-chapter markers.

### Steps
1. **Voice registry:** DB-backed `voice` table `{ id, provider: "xai", provider_voice_id, display_name, gender, accent, language, sample_url, enabled, premium_only }`. Seeded with Eve, Ara, Rex, Sal, Leo. Admin-editable; can disable voices or upload samples.
2. **x.ai client:** WebSocket to `wss://api.x.ai/v1/realtime`. Server-side only uses the full API key (never expose to browser). Send `session.update` with chosen voice + audio format (PCM 24 kHz default) + VAD disabled (we're driving, not conversing).
3. **TTS drive loop per chapter:**
   - Open WS session; send `conversation.item.create` with the chapter text (split into ~4 KB segments to stay comfortably under any single-message limits).
   - Send `response.create` with modality `audio`; collect streamed `audio.delta` PCM frames into a buffer.
   - On `response.done`, close session.
4. **Multi-voice narration (novel idea):** chapter text is parsed for speaker tags. The narrator voice reads default lines; character lines use assigned voices (user picks per character during preview). Each segment is generated separately and stitched; timing is preserved with short silence padding.
5. **Post-processing pipeline** (`core::audio`):
   - Normalise loudness to EBU R128 −16 LUFS (podcast standard) via `ffmpeg-next` bindings.
   - Insert inter-paragraph pauses (300 ms) and chapter gaps (1.2 s).
   - Optional background ambience: sidechain-duck a selected ambient track under narration at −22 dB. Tracks live in `/storage/ambience/` and are admin-managed.
   - Encode each chapter to Opus (48 kHz, 64 kbps) for streaming + concatenate into an M4B with chapter markers for download.
6. **Storage:**
   - `/storage/audio/<audiobook_id>/ch-<n>.opus` (per-chapter streaming)
   - `/storage/audio/<audiobook_id>/full.m4b` (download)
   - `/storage/audio/<audiobook_id>/waveform.json` (pre-computed peaks for wavesurfer.js → no client-side decode)
7. **Resume on failure:** if chapter 7 of 12 fails, jobs table records `last_ok_chapter=6`; the retry picks up from chapter 7 rather than restarting.
8. **Cost tracking:** record TTS seconds per voice per chapter, rolled up for quota + billing.

### Done when
- A 3-chapter test book renders to Opus + M4B with correct chapter metadata playable in VLC, Overcast, and the web player.
- Killing the backend mid-generation and restarting resumes from the last completed chapter.

---

## Phase 5 — Job Orchestration & Real-Time Progress

**Goal:** durable, observable, restartable generation jobs, with live progress to the client.

### Steps
1. **`job` table:** `{ id, kind: "outline|chapters|tts|postprocess|cover", audiobook_id, status, progress_pct, attempts, last_error?, queued_at, started_at, finished_at, worker_id, payload_json }`.
2. **Worker runtime:** in-process Tokio tasks with a bounded concurrency per job kind (e.g. max 4 TTS jobs in parallel). Jobs are picked by atomic `UPDATE ... SET worker_id=... WHERE status='queued' RETURN ...` in SurrealDB.
3. **Chapter-level parallelism (novel idea):** a single audiobook's TTS job fans out one sub-job per chapter for up to 4× speed-up; the parent job aggregates progress.
4. **WS progress hub:** `GET /ws/audiobook/:id` upgrades to WebSocket and streams `{ type: "progress", stage, chapter, pct, eta_seconds }` events. Backend uses `tokio::sync::broadcast` channels keyed by audiobook id.
5. **Backpressure:** if a user queues more jobs than their tier allows, additional jobs get `status=throttled` with a human-readable reason.
6. **Idempotency:** every mutating client request accepts `Idempotency-Key` and is de-duplicated for 24 h via a `request_idempotency` table.
7. **Dead-letter:** jobs that exceed `max_attempts` land in `status=dead` and are visible in the admin panel with replay button.
8. **Scheduled tasks:** nightly job to garbage-collect soft-deleted audiobooks older than 30 days and purge orphaned audio files.

### Done when
- Creating an audiobook end-to-end fires outline → chapters → tts → postprocess jobs in order, with live progress visible in the web UI.
- Restarting the backend mid-flight resumes without data loss.

---

## Phase 6 — Web Frontend

**Goal:** beautiful, responsive web app covering the entire user flow.

### Steps
1. **Foundation:**
   - Vite + React 18 + TS strict mode, TanStack Router file-based routes, Zustand for client state, TanStack Query for server state.
   - Tailwind configured with shadcn/ui + Radix; dark mode via `class` strategy, persisted in `localStorage`, synced with system preference.
   - Generated TS API client from backend's OpenAPI via `openapi-typescript-codegen` in CI (`npm run gen:api`).
2. **Auth pages:** `/login`, `/signup`, `/forgot`, `/verify-email`. Social login buttons via OAuth redirect.
3. **Dashboard `/app`:** library grid (cover + title + progress ring), "Create new audiobook" CTA, filters (status, genre, recently played).
4. **Create wizard `/app/new`:** 4 steps, each a route child:
   1. **Topic** — free-form text input + "Surprise me" button (calls random-topic endpoint) + example chips.
   2. **Style** — length (short/medium/long), genre, tone sliders (formal↔playful, dense↔light).
   3. **Voices** — primary narrator + (optional) per-character voice assignment with inline sample previews.
   4. **Review & Generate** — shows estimated cost in tokens + TTS seconds + time to completion, asks for confirmation.
5. **Generation view `/app/book/:id`:** WebSocket-driven progress UI. Chapter cards flip from "queued" → "writing" → "voicing" → "ready". Users can read chapter text while audio is still being generated.
6. **Player `/app/play/:id`:**
   - Custom wavesurfer.js-based player with pre-fetched `waveform.json` (no client decode cost).
   - Controls: play/pause, ±15 s skip, speed (0.75×, 1×, 1.25×, 1.5×, 2×), sleep timer, chapter list.
   - Bookmarks (persistent), notes per timestamp (novel idea — great for study material).
   - Keyboard shortcuts (`space`, `j`, `l`, `,`, `.`, numbers for chapters).
   - Progress synced via `PATCH /audiobook/:id/progress` throttled to once every 10 s.
7. **Library `/app/library`:** list + grid toggle; bulk actions (delete, export M4B, share).
8. **Accessibility:** all Radix primitives; full keyboard nav; SR labels; contrast AA; captions/subtitles view synchronised with playback (SRT produced alongside audio — novel idea, Phase 4 add-on).
9. **PWA (novel idea):** service worker pre-caches chapter Opus files on demand so the web app works offline, matching the iOS "Download for offline listening" capability.
10. **Cover art:** generated via the `flux2` skill at text-ready checkpoint; user can regenerate with a new prompt.

### Done when
- A new visitor can sign up, generate a short audiobook end-to-end, and play it in the browser, including offline playback.
- Lighthouse ≥ 90 on Performance, Accessibility, Best Practices.

---

## Phase 7 — Admin Panel

**Goal:** a `/admin` zone of the same web app, gated by `role=admin`, covering all runtime-editable entities.

### Steps
1. **Layout:** sidebar navigation: LLMs, Voices, Prompt Templates, Users, Content, Jobs, Pricing & Tiers, Feature Flags, System Health.
2. **LLM management:** CRUD on `llm` rows, live test (send a ping prompt), enable/disable, set defaults per role.
3. **Voice management:** CRUD on `voice` rows, inline sample player, upload/record custom preview clips, pair voices into "voice packs" (e.g. "Noir Detective: male gruff narrator + female femme-fatale").
4. **Prompt template editor:** diff-aware editor (Monaco), version history, A/B test between two versions with metric = average user rating.
5. **User management:** search, filter by tier, change role, revoke sessions, apply quota overrides, masquerade (novel idea — log in as user for support, logged in audit trail).
6. **Content moderation queue:** surfaces audiobooks flagged by safety pass; admin can approve, soft-delete, or contact user.
7. **Jobs board:** live table of running/dead jobs with retry buttons.
8. **Pricing & tiers:** edit tier limits and prices (Stripe-synced — Phase 9).
9. **Feature flags:** simple boolean/percentage flags from DB, consumed by both backend and frontend.
10. **System health:** DB size, job throughput, OpenRouter + x.ai error rates, 7-day cost breakdown.

### Done when
- Every configurable thing in the system is editable without a redeploy.
- An admin can ban a user, refund their quota, disable a broken LLM, and retry a dead job in under 2 minutes.

---

## Phase 8 — iOS App

**Goal:** native iOS client sharing the backend.

### Steps
1. **Project:** Xcode 15, Swift 5.10, SwiftUI, target iOS 16+.
2. **Networking:** generated Swift API client from the same OpenAPI spec (`swift-openapi-generator`).
3. **Auth:** Sign in with Apple + email/password + Google; refresh-token rotation matches web.
4. **Screens:** Login → Library → Create Wizard → Player.
5. **Audio:** `AVAudioEngine` + `AVQueuePlayer` with Now Playing info (title, cover), remote commands (play/pause/skip), CarPlay support.
6. **Offline:** Core Data cache of audiobooks + downloaded chapter files stored in the app group so extensions can read.
7. **Background download:** `URLSession` background configuration for downloading on lock screen.
8. **Progress sync:** updates server via same endpoints; last-writer-wins with vector-clock fallback when device is offline.
9. **Notifications:** push (APNs) for "your audiobook is ready" after long jobs.
10. **App Store assets:** screenshots, description, privacy labels, TestFlight beta.

### Done when
- App passes TestFlight internal review, supports CarPlay, and plays downloaded audiobooks in airplane mode.

---

## Phase 9 — Monetisation, Quotas, and Billing

**Goal:** sustainable economics — free tier + premium tier + optional pay-as-you-go.

### Steps
1. **Stripe integration:** `stripe-rust` crate. Products for Free (0), Pro (monthly), and top-up packs (e.g. +5 hours TTS).
2. **Webhook handler:** `/webhooks/stripe` verifies signatures and updates `user.tier` + `user.billing_period_reset_at`.
3. **Quota enforcement:** central `QuotaService` consulted before outline, chapter, and TTS jobs; produces friendly error + upgrade CTA.
4. **Usage dashboard** in the settings page: current-period consumption vs. tier, historical monthly chart.
5. **Referrals (novel idea):** signed referral links grant both referrer and referee +60 TTS minutes; anti-abuse via device + IP fingerprinting + same-card detection.
6. **Gifting (novel idea):** buy a 1-hour audiobook credit for someone else by email; recipient redeems on signup.

### Done when
- Can upgrade to Pro via Stripe Checkout, see new quota immediately, and downgrade without losing existing audiobooks.

---

## Phase 10 — Polish, Hardening, Launch

**Goal:** ready for real users.

### Steps
1. **Security review:** run `cargo audit`, `npm audit`, OWASP ZAP baseline scan. Add CSP, HSTS, secure cookie flags.
2. **Rate limiting:** per-endpoint + per-user, using a token bucket in SurrealDB (or Redis if we outgrow single-node).
3. **Backups:** nightly SurrealDB export + audio bucket sync to off-box storage. Tested restore script.
4. **Observability:** Grafana dashboard (via Prometheus scrape or OTEL) for request latency, job throughput, provider error rates, cost per hour.
5. **Load test:** k6 script simulating 100 concurrent users generating short books; verify backend gracefully throttles rather than dropping jobs.
6. **Legal:** Terms, Privacy Policy, DPA, data-export + account-deletion endpoints (GDPR), cookie banner, x.ai + OpenRouter ToS compliance review (especially re: redistribution of generated audio).
7. **Docs site:** brief user guide + API reference (Redocly from OpenAPI).
8. **Public launch checklist:**
   - Domain + TLS (Caddy or Nginx)
   - Email deliverability (SPF/DKIM/DMARC)
   - Sentry for error tracking
   - Status page
   - Analytics (self-hosted Plausible)
   - Pre-generated showcase library of 20 audiobooks across genres

### Done when
- All checklist items ticked, incident runbook exists, on-call rotation set, v1.0 tagged.

---

## Cross-Cutting Novel Ideas (added on top of the brief)

These are lightweight additions that make the product noticeably better; each is small and can slot into an existing phase.

| Idea | Phase | Value |
|------|-------|-------|
| Multi-voice narration with per-character voices | 4, 6 | Turns flat TTS into immersive theatre. |
| Chapter-level parallel TTS | 5 | 3–4× faster generation for long books. |
| SRT/VTT subtitle track alongside audio | 4, 6 | Accessibility + scripture-style study use. |
| Timestamped listener notes | 6 | Makes the app useful for study/non-fiction. |
| PWA offline playback on web | 6 | Parity with native without shipping native. |
| Vector search for "similar audiobooks" | 5, 6 | Uses SurrealDB's vector index on chapter embeddings. |
| A/B testable prompt templates with rating feedback | 3, 7 | Continuously improve quality without redeploys. |
| Masquerade login for support | 7 | Reduces support time drastically. |
| Gifting + referral credits | 9 | Organic growth levers. |
| Pre-generated showcase library | 10 | Empty-library problem solved at launch. |
| M4B export with chapter markers | 4 | Pro users can listen in Audible-style apps. |
| Safe-content moderation gates | 3 | Avoids legal + reputational issues. |
| Admin A/B-testable prompts | 3, 7 | Prompt quality is product quality; admin should own it. |

---

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| x.ai voice API pricing changes or rate limits | Thin `TtsClient` trait so we can add ElevenLabs or Azure TTS quickly. Per-job cost caps. |
| SurrealDB single-node limits at scale | Design jobs + data model to be horizontally shardable (idempotent job ids, no cross-user joins). Migration path to TiKV backend is documented. |
| LLM output violating JSON schema | Repair-retry loop + structured-output mode where available (OpenRouter routes to models that support it). |
| Generation runs are expensive if user abandons | Soft-cancel on 10 min idle during wizard; show cost estimate before "Generate". |
| Copyright / trademarked topics | Moderation pass + rejected topics list + ToS clarity. |
| iOS App Store review (AI content policies) | Clear content guidelines, reporting flow, age gating. |

---

## Suggested First Milestones (Week-by-Week)

- **Week 1:** Phase 0 + Phase 1 foundation.
- **Week 2:** Phase 2 auth.
- **Week 3:** Phase 3 outline + single-chapter generation (no narration yet).
- **Week 4:** Phase 4 single-voice TTS end-to-end; ugly web player.
- **Week 5:** Phase 5 jobs + live progress.
- **Weeks 6–7:** Phase 6 real web UI.
- **Week 8:** Phase 7 admin panel.
- **Weeks 9–11:** Phase 8 iOS.
- **Week 12:** Phase 9 billing.
- **Weeks 13–14:** Phase 10 polish + launch.

---

## Open Questions for the Owner

1. Is the "long" length (up to ~5 hours of audio) in scope for v1, or should we cap at 30 min to control early costs?
2. Do we want user-uploaded reference voices (voice cloning) as a roadmap item? x.ai does not expose this today; would require adding ElevenLabs.
3. Should the web app's admin panel and user app share a single deployment, or live at `admin.listenai.app` separately?
4. Hosting preference: single VPS (Hetzner) for v1 vs. managed (Fly.io, Railway) vs. Cloudflare Workers + R2 (would require moving off embedded SurrealDB)?
5. Desired free-tier generosity — e.g. 30 minutes of audio/month free?

Answers to these will refine Phases 8–10.
