set shell := ["bash", "-cu"]
set dotenv-load := true

default:
    @just --list

# --- Install / setup -------------------------------------------------------

setup:
    cd frontend && npm install

# --- Dev -------------------------------------------------------------------

# Run backend (hot reload requires cargo-watch: `cargo install cargo-watch`)
dev-backend:
    cd backend && cargo run -p api

dev-backend-watch:
    cd backend && cargo watch -x 'run -p api'

dev-frontend:
    cd frontend && npm run dev

# Run both concurrently (requires GNU parallel or two terminals in practice;
# easiest is two shells, but this works if you have `concurrently` via npx).
dev:
    npx --yes concurrently -k -n backend,frontend -c blue,magenta "just dev-backend" "just dev-frontend"

# --- Quality ---------------------------------------------------------------

fmt:
    cd backend && cargo fmt --all
    cd frontend && npm run format

lint:
    cd backend && cargo fmt --all -- --check
    cd backend && cargo clippy --all-targets --all-features -- -D warnings
    cd frontend && npm run lint

typecheck:
    cd frontend && npm run typecheck

test:
    cd backend && cargo test --all
    cd frontend && npm test --if-present

check: lint typecheck test

# --- Build -----------------------------------------------------------------

build:
    cd backend && cargo build --release
    cd frontend && npm run build

clean:
    cd backend && cargo clean
    rm -rf frontend/dist frontend/node_modules/.vite
