# syntax=docker/dockerfile:1.7
#
# AidBooks API — production image.
#
# Multi-stage build:
#   1. `builder` compiles the `listenai-api` binary against the workspace.
#      RocksDB (pulled in by surrealdb's `kv-rocksdb` feature) is built from
#      vendored sources, so we need clang + cmake + libclang + the standard
#      build toolchain. The build is single-shot — for incremental rebuilds
#      podman/docker reuses the cached intermediate layers as long as the
#      Cargo manifests haven't changed.
#   2. `runtime` is a slim debian image with only the binary, ffmpeg (used
#      by the YouTube publisher to mux WAVs into MP4), and ca-certificates
#      (TLS roots for OpenRouter / xAI / YouTube).
#
# Animation/Manim are intentionally omitted from this image — those pipelines
# need Chromium + Node + LaTeX + Cairo + Pango, which roughly triples the
# image size. The api still boots fine; animate jobs simply refuse to run
# because LISTENAI_ANIMATE_RENDERER_CMD is empty.
#
# Build context must be the repo root (so /backend and /frontend are siblings
# under the build context). The deploy script enforces this.

ARG RUST_VERSION=1.94.1
ARG DEBIAN_VERSION=bookworm

# ---------------------------------------------------------------------------
# Stage 1: builder
# ---------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-${DEBIAN_VERSION} AS builder

ENV CARGO_TERM_COLOR=always \
    DEBIAN_FRONTEND=noninteractive

# rocksdb-sys needs cmake + clang + libclang to build the vendored librocksdb.
# pkg-config + libssl-dev are belt-and-braces in case any sub-dep wants the
# system OpenSSL (reqwest itself is rustls-only in this workspace).
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang \
        cmake \
        libclang-dev \
        pkg-config \
        libssl-dev \
        build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy the workspace manifests + every member's Cargo.toml first so the
# expensive dependency-fetch layer is reused whenever only Rust source
# changes. The full workspace tree is needed for cargo to resolve paths.
COPY backend/ ./backend/
COPY rust-toolchain.toml ./

# Single release build of just the api crate. `--locked` makes the build
# reproducible and fails loudly if Cargo.lock drifts.
RUN cd backend && cargo build --release --locked -p api

# ---------------------------------------------------------------------------
# Stage 2: runtime
# ---------------------------------------------------------------------------
FROM debian:${DEBIAN_VERSION}-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive

# - ffmpeg: required at runtime by the YouTube publisher (`ffmpeg_bin`).
# - ca-certificates: TLS roots for outbound HTTPS (OpenRouter, xAI, YouTube).
# - libssl3 / libstdc++6 / libgcc-s1: surfaced by the linked binary; provided
#   by debian-slim already but we include them explicitly for clarity.
# - tini: PID-1 signal handling so SIGTERM from k8s actually reaches the
#   process and the graceful-shutdown path runs.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        ffmpeg \
        libssl3 \
        libstdc++6 \
        libgcc-s1 \
        tini \
    && rm -rf /var/lib/apt/lists/*

# Run as a non-root user. UID 1000 matches typical k8s `runAsUser` values.
RUN groupadd --system --gid 1000 listenai \
    && useradd --system --uid 1000 --gid listenai --create-home --home-dir /home/listenai listenai

WORKDIR /app

COPY --from=builder /build/backend/target/release/listenai-api /usr/local/bin/listenai-api

# /data is the PVC mount point; the api writes its embedded SurrealDB and
# generated audio/video here. We chown so the non-root user can write
# without an extra `fsGroup` dance — k8s `securityContext.fsGroup` is still
# the right move in the deployment manifest, but this gives us a fallback.
RUN mkdir -p /data/db /data/audio \
    && chown -R listenai:listenai /data /app

USER listenai

EXPOSE 8787

# tini swallows SIGTERM correctly and reaps zombies if the api ever spawns
# child processes (e.g. ffmpeg for the publisher).
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/listenai-api"]
