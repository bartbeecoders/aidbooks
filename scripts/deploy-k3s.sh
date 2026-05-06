#!/bin/bash
set -euo pipefail

#===============================================================================
# AidBooks - K3S Deployment Script (Podman)
#===============================================================================
# Builds and pushes the api + frontend images, then deploys to K3S on the VPS.
#
# Architecture:
#   • aidbooks-api      Rust axum + embedded SurrealDB. ClusterIP only.
#   • aidbooks-frontend nginx + Vite SPA. NodePort 32085 — Cloudflare Tunnel
#                       forwards https://aidbooks.hideterms.com here.
#   • aidbooks-data     20Gi PVC (local-path) holding the embedded DB and
#                       generated audio/video files.
#
# Namespace: aidbooks
# Registry:  beecodersregistry.azurecr.io  (reuses the existing acr-secret
#            ImagePullSecret in the cluster — applied per-namespace below.)
#
# Usage:
#   ./scripts/deploy-k3s.sh [all|build|push|deploy|secret|status|logs]
#
#   all     build → push → deploy → status (default)
#   build   build api + frontend images locally with podman
#   push    podman login + push both images
#   deploy  scp manifests + apply on the VPS, restart deployments
#   secret  copy k8s/aidbooks/secret.yaml to the VPS and apply it. Run this
#           once after editing secret.yaml; `deploy` does NOT touch secrets.
#   status  kubectl get on the namespace
#   logs    tail api + frontend logs
#
# Required tools locally:
#   podman, ssh, scp
#
# First-time setup (run once on the VPS or via this script's `secret`
# command):
#   1. Ensure the namespace exists:           kubectl apply -f k8s/aidbooks/namespace.yaml
#   2. Apply the imagePullSecret in the ns:   see acr_secret() below.
#   3. Apply the env secret:                  ./scripts/deploy-k3s.sh secret
#===============================================================================

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

COMMAND="${1:-all}"

REGISTRY="${REGISTRY:-beecodersregistry.azurecr.io}"
NAMESPACE="aidbooks"
API_IMAGE="$REGISTRY/aidbooks-api"
FRONTEND_IMAGE="$REGISTRY/aidbooks-frontend"

# VPS target (override via environment variables).
VPS_IP="${VPS_IP:-212.47.77.32}"
VPS_USER="${VPS_USER:-bart}"

VPS_BASE_DIR="${VPS_BASE_DIR:-~/aidbooks}"
VPS_K8S_DIR="$VPS_BASE_DIR/k8s"

# Single source of truth for the image tag. Bumping the workspace version
# (backend/Cargo.toml) automatically tags both images. The script also
# always pushes :latest so the rolling update picks it up regardless.
APP_VERSION=$(grep -m1 -E '^version' "$ROOT_DIR/backend/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')
if [[ -z "$APP_VERSION" ]]; then
  echo "could not parse version from backend/Cargo.toml workspace.package"
  exit 1
fi

ssh_vps() {
  local cmd="$1"
  ssh -o StrictHostKeyChecking=accept-new "$VPS_USER@$VPS_IP" "bash -lc $(printf %q "$cmd")"
}

kubectl_vps() {
  local args="$1"
  ssh_vps "if command -v kubectl >/dev/null 2>&1; then kubectl $args; else sudo k3s kubectl $args; fi"
}

check_build_deps() {
  command -v podman >/dev/null 2>&1 || { echo "podman not found"; exit 1; }
}

check_remote_deps() {
  command -v ssh >/dev/null 2>&1 || { echo "ssh not found"; exit 1; }
  command -v scp >/dev/null 2>&1 || { echo "scp not found"; exit 1; }
}

# -----------------------------------------------------------------------------
# Build
# -----------------------------------------------------------------------------
build_api() {
  echo "==> Building $API_IMAGE:$APP_VERSION"
  podman build \
    --pull=newer \
    -t "$API_IMAGE:latest" \
    -t "$API_IMAGE:$APP_VERSION" \
    -f "$ROOT_DIR/backend/Dockerfile" \
    "$ROOT_DIR"
}

build_frontend() {
  echo "==> Building $FRONTEND_IMAGE:$APP_VERSION"
  podman build \
    --pull=newer \
    -t "$FRONTEND_IMAGE:latest" \
    -t "$FRONTEND_IMAGE:$APP_VERSION" \
    -f "$ROOT_DIR/frontend/Dockerfile" \
    "$ROOT_DIR"
}

build_images() {
  build_api
  build_frontend
}

# -----------------------------------------------------------------------------
# Push
# -----------------------------------------------------------------------------
push_images() {
  echo "==> Logging into $REGISTRY"
  if [[ -n "${REGISTRY_USER:-}" && -n "${REGISTRY_PASSWORD:-}" ]]; then
    podman login -u "$REGISTRY_USER" -p "$REGISTRY_PASSWORD" "$REGISTRY"
  else
    podman login "$REGISTRY"
  fi

  echo "==> Pushing $API_IMAGE"
  podman push "$API_IMAGE:latest"
  podman push "$API_IMAGE:$APP_VERSION"

  echo "==> Pushing $FRONTEND_IMAGE"
  podman push "$FRONTEND_IMAGE:latest"
  podman push "$FRONTEND_IMAGE:$APP_VERSION"
}

# -----------------------------------------------------------------------------
# Deploy
# -----------------------------------------------------------------------------
ensure_remote_dirs() {
  ssh_vps "mkdir -p $VPS_K8S_DIR || (command -v sudo >/dev/null 2>&1 && sudo mkdir -p $VPS_K8S_DIR && sudo chown -R $VPS_USER:$VPS_USER $VPS_BASE_DIR)"
}

copy_manifests() {
  ensure_remote_dirs
  # Copy the whole aidbooks dir so namespace/configmap/pvc/deployment land
  # on the VPS together. We *don't* copy secret.yaml here — it goes via
  # the dedicated `secret` command so a routine deploy can't accidentally
  # overwrite a hand-edited secret.
  scp -o StrictHostKeyChecking=accept-new -r \
    "$ROOT_DIR/k8s/aidbooks" \
    "$VPS_USER@$VPS_IP:$VPS_K8S_DIR/"
}

deploy_manifests() {
  echo "==> Deploying to $VPS_USER@$VPS_IP (namespace: $NAMESPACE)"
  copy_manifests

  # 1. Namespace must exist before anything else lands in it.
  kubectl_vps "apply -f $VPS_K8S_DIR/aidbooks/namespace.yaml"

  # 2. ImagePullSecret. We assume `acr-secret` is already present in some
  # namespace (it was, for sqail). If it's not in aidbooks yet, copy it
  # from sqail. This is a one-shot but cheap to re-run.
  ensure_acr_secret

  # 3. Confirm the env Secret exists. Without it the api crashes on boot
  # (LISTENAI_JWT_SECRET is required). Fail fast with a clear message.
  if ! kubectl_vps "-n $NAMESPACE get secret aidbooks-api-secret >/dev/null 2>&1"; then
    echo ""
    echo "ERROR: secret 'aidbooks/aidbooks-api-secret' is missing."
    echo "       Run: ./scripts/deploy-k3s.sh secret"
    echo "       (after copying secret.example.yaml → secret.yaml and filling it in)"
    exit 1
  fi

  # 4. Config + storage + workloads. Order matters: PVC before deployment
  # so the volume bind doesn't race the pod start.
  kubectl_vps "apply -f $VPS_K8S_DIR/aidbooks/configmap.yaml"
  kubectl_vps "apply -f $VPS_K8S_DIR/aidbooks/pvc.yaml"
  kubectl_vps "apply -f $VPS_K8S_DIR/aidbooks/service-api.yaml"
  kubectl_vps "apply -f $VPS_K8S_DIR/aidbooks/service-frontend.yaml"
  kubectl_vps "apply -f $VPS_K8S_DIR/aidbooks/deployment-api.yaml"
  kubectl_vps "apply -f $VPS_K8S_DIR/aidbooks/deployment-frontend.yaml"

  # 5. Force a rollout so an unchanged tag still picks up the new image
  # digest (we always push :latest with imagePullPolicy: Always).
  kubectl_vps "-n $NAMESPACE rollout restart deployment aidbooks-api"
  kubectl_vps "-n $NAMESPACE rollout restart deployment aidbooks-frontend"
  kubectl_vps "-n $NAMESPACE rollout status deployment aidbooks-api --timeout=180s"
  kubectl_vps "-n $NAMESPACE rollout status deployment aidbooks-frontend --timeout=120s"

  echo ""
  echo "Deployed AidBooks v$APP_VERSION"
  echo "  • NodePort:         http://$VPS_IP:32085   (cloudflare tunnel target)"
  echo "  • Public hostname:  https://aidbooks.hideterms.com"
  echo "  • API health:       kubectl -n aidbooks exec deploy/aidbooks-frontend -- wget -qO- http://aidbooks-api:8787/health"
}

# Copy the existing acr-secret from any namespace that has one (sqail's the
# obvious source) into aidbooks. Idempotent — does nothing if it's already
# in aidbooks. Bails if no source can be found, with a clear remediation
# hint.
ensure_acr_secret() {
  if kubectl_vps "-n $NAMESPACE get secret acr-secret >/dev/null 2>&1"; then
    return 0
  fi

  echo "==> aidbooks/acr-secret missing; copying from sqail/acr-secret"
  if kubectl_vps "-n sqail get secret acr-secret >/dev/null 2>&1"; then
    # `kubectl get -o yaml | sed | kubectl apply` is the canonical
    # cross-namespace copy. Strip namespace + resourceVersion + uid so
    # apply is happy.
    kubectl_vps "-n sqail get secret acr-secret -o yaml \
      | sed -e '/namespace:/d' -e '/resourceVersion:/d' -e '/uid:/d' -e '/creationTimestamp:/d' \
      | kubectl -n $NAMESPACE apply -f -"
  else
    echo ""
    echo "ERROR: no source acr-secret found (looked in 'sqail' namespace)."
    echo "       Create it manually:"
    echo "         kubectl -n $NAMESPACE create secret docker-registry acr-secret \\"
    echo "           --docker-server=$REGISTRY \\"
    echo "           --docker-username=<acr-user> \\"
    echo "           --docker-password=<acr-token>"
    exit 1
  fi
}

# -----------------------------------------------------------------------------
# Secret (separate from `deploy` on purpose — see header comment).
# -----------------------------------------------------------------------------
apply_secret() {
  local local_secret="$ROOT_DIR/k8s/aidbooks/secret.yaml"
  if [[ ! -f "$local_secret" ]]; then
    echo "ERROR: $local_secret not found."
    echo "       cp k8s/aidbooks/secret.example.yaml k8s/aidbooks/secret.yaml"
    echo "       # then edit and re-run"
    exit 1
  fi

  ensure_remote_dirs
  scp -o StrictHostKeyChecking=accept-new \
    "$local_secret" \
    "$VPS_USER@$VPS_IP:$VPS_K8S_DIR/aidbooks/secret.yaml"

  # Make sure namespace exists, then apply.
  kubectl_vps "apply -f $VPS_K8S_DIR/aidbooks/namespace.yaml"
  kubectl_vps "apply -f $VPS_K8S_DIR/aidbooks/secret.yaml"

  # Restart so the api re-reads env. envFrom is evaluated at pod start.
  if kubectl_vps "-n $NAMESPACE get deployment aidbooks-api >/dev/null 2>&1"; then
    kubectl_vps "-n $NAMESPACE rollout restart deployment aidbooks-api"
  fi

  # Drop the secret from the VPS once applied — k8s now owns it.
  ssh_vps "rm -f $VPS_K8S_DIR/aidbooks/secret.yaml"

  echo "Secret applied (and removed from disk on the VPS)."
}

# -----------------------------------------------------------------------------
# Status / logs
# -----------------------------------------------------------------------------
status() {
  kubectl_vps "-n $NAMESPACE get pods,svc,pvc,deploy"
}

logs() {
  echo "==> aidbooks-api (last 100 lines)"
  kubectl_vps "-n $NAMESPACE logs deployment/aidbooks-api --tail=100" || true
  echo ""
  echo "==> aidbooks-frontend (last 50 lines)"
  kubectl_vps "-n $NAMESPACE logs deployment/aidbooks-frontend --tail=50" || true
}

# -----------------------------------------------------------------------------
# main
# -----------------------------------------------------------------------------
main() {
  case "$COMMAND" in
    all)
      check_build_deps
      check_remote_deps
      echo "Deploying AidBooks v$APP_VERSION to $VPS_USER@$VPS_IP"
      build_images
      push_images
      deploy_manifests
      status
      ;;
    build)          check_build_deps; build_images ;;
    build-api)      check_build_deps; build_api ;;
    build-frontend) check_build_deps; build_frontend ;;
    push)           check_build_deps; push_images ;;
    deploy)         check_remote_deps; deploy_manifests ;;
    secret)         check_remote_deps; apply_secret ;;
    status)         check_remote_deps; status ;;
    logs)           check_remote_deps; logs ;;
    *)
      echo "Unknown command: $COMMAND"
      echo "Usage: $0 [all|build|build-api|build-frontend|push|deploy|secret|status|logs]"
      exit 1
      ;;
  esac
}

main
