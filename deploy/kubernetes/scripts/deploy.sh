#!/usr/bin/env bash
# Deploy Edgion Gateway to a Kubernetes cluster.
#
# Usage:
#   ./deploy.sh [OPTIONS]
#
# Options:
#   --skip-crd           Skip CRD installation
#   --skip-base-config   Skip applying base GatewayClass/Gateway config
#   --no-wait            Do not wait for rollout to complete
#   -y, --yes            Skip confirmation prompt

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

SKIP_CRD=false
SKIP_BASE_CONFIG=false
WAIT_READY=true
YES=false

VERSIONS_FILE="${ROOT_DIR}/versions.env"

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) sed -n '/^# /s/^# //p' "$0"; exit 0 ;;
    --skip-crd) SKIP_CRD=true; shift ;;
    --skip-base-config) SKIP_BASE_CONFIG=true; shift ;;
    --no-wait) WAIT_READY=false; shift ;;
    -y|--yes) YES=true; shift ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

command -v kubectl >/dev/null 2>&1 || { echo "kubectl not found" >&2; exit 1; }

if [[ -f "${VERSIONS_FILE}" ]]; then
  # shellcheck disable=SC1090
  source "${VERSIONS_FILE}"
fi

CONTROLLER_IMAGE="${CONTROLLER_IMAGE:-${DEFAULT_CONTROLLER_IMAGE:-docker.io/pandaala/edgion-controller:0.1.5}}"
GATEWAY_IMAGE="${GATEWAY_IMAGE:-${DEFAULT_GATEWAY_IMAGE:-docker.io/pandaala/edgion-gateway:0.1.5}}"

echo "═══════════════════════════════════════════════════"
echo "  Edgion Gateway — Deployment Plan"
echo "═══════════════════════════════════════════════════"
echo "  Cluster:      $(kubectl config current-context 2>/dev/null || echo '(unknown)')"
echo "  Controller:   ${CONTROLLER_IMAGE}"
echo "  Gateway:      ${GATEWAY_IMAGE}"
echo "  CRD install:  $([ "${SKIP_CRD}" == true ] && echo skip || echo yes)"
echo "  Base config:  $([ "${SKIP_BASE_CONFIG}" == true ] && echo skip || echo yes)"
echo "═══════════════════════════════════════════════════"

if [[ "${YES}" == false ]]; then
  read -r -p "Continue? [y/N] " REPLY
  echo
  [[ "${REPLY}" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 0; }
fi

step=1

echo "[${step}] Apply namespace"
kubectl apply -f "${ROOT_DIR}/namespace.yaml"
(( step++ ))

# Ensure the admin-credentials Secret exists before the controller starts.
# This is idempotent: if the Secret already exists (e.g. from a previous deployment or
# the old auto-generation), it is left untouched so the current password is preserved.
# The controller reads credentials from this Secret via env vars (secretKeyRef).
echo "[${step}] Ensure admin credentials Secret"
kubectl -n edgion-system get secret edgion-admin-credentials >/dev/null 2>&1 || \
  kubectl -n edgion-system create secret generic edgion-admin-credentials \
    --from-literal=username=admin \
    --from-literal=password="$(openssl rand -base64 16)" \
    --from-literal=jwt_secret="$(openssl rand -base64 32)"
(( step++ ))

# Ensure the gateway admin API token Secret exists before the gateway starts.
# Kept independent of the controller credentials so the two rotate separately.
# Idempotent: if the Secret already exists, it is left untouched so the current
# token is preserved. The gateway reads the token via secretKeyRef (env var).
echo "[${step}] Ensure gateway admin token Secret"
kubectl -n edgion-system get secret edgion-gateway-admin-credentials >/dev/null 2>&1 || \
  kubectl -n edgion-system create secret generic edgion-gateway-admin-credentials \
    --from-literal=token="$(openssl rand -hex 32)"
(( step++ ))

if [[ "${SKIP_CRD}" == false ]]; then
  echo "[${step}] Install CRDs"
  "${SCRIPT_DIR}/install_crds.sh"
else
  echo "[${step}] Skip CRD installation (--skip-crd)"
fi
(( step++ ))

echo "[${step}] Apply controller"
kubectl apply -f "${ROOT_DIR}/controller/"
kubectl set image deployment/edgion-controller -n edgion-system \
  edgion-controller="${CONTROLLER_IMAGE}"
(( step++ ))

echo "[${step}] Apply gateway"
kubectl apply -f "${ROOT_DIR}/gateway/"
kubectl set image deployment/edgion-gateway -n edgion-system \
  edgion-gateway="${GATEWAY_IMAGE}"
(( step++ ))

if [[ "${SKIP_BASE_CONFIG}" == false ]]; then
  echo "[${step}] Apply base config (GatewayClass, EdgionGatewayConfig, Gateways)"
  kubectl apply -f "${ROOT_DIR}/base-config/"
else
  echo "[${step}] Skip base config (--skip-base-config)"
fi
(( step++ ))

if [[ "${WAIT_READY}" == true ]]; then
  echo "Waiting for controller rollout..."
  kubectl rollout status deployment/edgion-controller -n edgion-system --timeout=300s
  echo "Waiting for gateway rollout..."
  kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=300s
fi

echo ""
echo "Deployment finished."
