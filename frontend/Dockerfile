# syntax=docker/dockerfile:1.7
#
# AidBooks frontend — production image.
#
# Multi-stage build:
#   1. `builder` runs `npm ci && npm run build` to produce frontend/dist.
#   2. `runtime` is nginx-alpine serving the SPA, with a custom config that
#      reverse-proxies /api/* (incl. /api/ws/*) to the api Service inside
#      the cluster. The frontend itself only ever talks to relative URLs,
#      so nothing else needs to know the backend address.
#
# Build context must be the repo root so the COPY paths resolve against
# the same context the api Dockerfile uses (the deploy script does this).

ARG NODE_VERSION=20
ARG NGINX_VERSION=1.27

# ---------------------------------------------------------------------------
# Stage 1: builder
# ---------------------------------------------------------------------------
FROM node:${NODE_VERSION}-alpine AS builder

WORKDIR /build

# Install deps first so this layer is cached when only source changes.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund

# Now bring in the rest of the source and the shared OpenAPI directory
# (the lint script imports types from ../shared, but `vite build` does
# not — keep the COPY scoped to frontend to avoid invalidating the
# dep-cache layer when shared/ changes).
COPY frontend/ ./

# Production build. Vite output lands in /build/dist.
RUN npm run build

# ---------------------------------------------------------------------------
# Stage 2: runtime
# ---------------------------------------------------------------------------
FROM nginx:${NGINX_VERSION}-alpine AS runtime

# Drop the default site config and replace with our SPA + /api proxy config.
RUN rm /etc/nginx/conf.d/default.conf
COPY frontend/nginx.conf /etc/nginx/conf.d/aidbooks.conf

COPY --from=builder /build/dist /usr/share/nginx/html

EXPOSE 80

# nginx images already have a sensible CMD; we keep it. STOPSIGNAL SIGQUIT
# is the upstream default and gives nginx a graceful shutdown window.
STOPSIGNAL SIGQUIT
