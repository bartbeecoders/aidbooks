# ListenAI

AI-powered audiobook generator. Turn a topic into a structured, narrated audiobook.

See [`plan.md`](./plan.md) for the full implementation plan and [`Vibecoding/instructions.md`](./Vibecoding/instructions.md) for the original brief.

## Repository layout

| Path        | Purpose                                                       |
|-------------|---------------------------------------------------------------|
| `backend/`  | Rust workspace (Axum REST API, SurrealDB embedded, jobs)      |
| `frontend/` | Web app (Vite + React + TypeScript + Tailwind + shadcn/ui)    |
| `ios/`      | Native iOS client (added in Phase 8)                          |
| `shared/`   | OpenAPI spec + generated clients                              |
| `storage/`  | Runtime files: embedded DB + audio output (gitignored)        |
| `docs/`     | Architecture decisions and design notes                       |
| `Vibecoding/` | Original project brief                                      |

## Prerequisites

- Rust 1.94+ (pinned via `rust-toolchain.toml`)
- Node 20+
- `just` (optional but recommended) — `cargo install just` or `pacman -S just`

## Quick start

```bash
cp .env.example .env          # fill in keys as phases require them
cd frontend && npm install && cd ..

# two shells
just dev-backend              # → http://127.0.0.1:8787/health
just dev-frontend             # → http://127.0.0.1:5173
```

Or run both at once:

```bash
just dev
```

## Quality

```bash
just fmt         # format everything
just check       # lint + typecheck + test
just build       # release build
```

## Status

**Phase 0 — Project Scaffolding** complete.
Phases 1–10 tracked in [`plan.md`](./plan.md).
