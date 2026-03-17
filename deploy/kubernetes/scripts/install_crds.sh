#!/usr/bin/env bash
# Install Gateway API + Edgion CRDs into the current cluster.
#
# Usage:
#   ./install_crds.sh [OPTIONS]
#
# Options:
#   --channel <standard|experimental>   Gateway API CRD channel (default: experimental)
#   --gateway-api-version <ver>         Gateway API version (default: v1.4.0)
#   --edgion-version <ver>              Edgion git ref for CRD download (default: main)
#   --delete                            Delete CRDs instead of applying

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
EDGION_CRD_DIR="${REPO_ROOT}/config/crd/edgion-crd"

CHANNEL="experimental"
GATEWAY_API_VERSION="v1.4.0"
DELETE=false

GATEWAY_API_BASE="https://github.com/kubernetes-sigs/gateway-api/releases/download"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --channel) CHANNEL="${2:?Missing value}"; shift 2 ;;
    --gateway-api-version) GATEWAY_API_VERSION="${2:?Missing value}"; shift 2 ;;
    --delete) DELETE=true; shift ;;
    -h|--help) sed -n '/^# /s/^# //p' "$0"; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

command -v kubectl >/dev/null 2>&1 || { echo "kubectl not found" >&2; exit 1; }

if [[ ! -d "${EDGION_CRD_DIR}" ]]; then
  echo "ERROR: Edgion CRD directory not found: ${EDGION_CRD_DIR}" >&2
  exit 1
fi

GATEWAY_API_URL="${GATEWAY_API_BASE}/${GATEWAY_API_VERSION}/${CHANNEL}-install.yaml"

if [[ "${DELETE}" == true ]]; then
  echo "Deleting Gateway API CRDs..."
  kubectl delete -f "${GATEWAY_API_URL}" --ignore-not-found=true || true
  echo "Deleting Edgion CRDs..."
  for f in "${EDGION_CRD_DIR}"/*.yaml; do
    kubectl delete -f "${f}" --ignore-not-found=true 2>/dev/null || true
  done
  echo "CRD deletion finished."
  exit 0
fi

echo "Installing Gateway API CRDs (${CHANNEL} ${GATEWAY_API_VERSION})"
kubectl apply --server-side --force-conflicts -f "${GATEWAY_API_URL}"

echo ""
echo "Installing Edgion CRDs (local: ${EDGION_CRD_DIR})"
for f in "${EDGION_CRD_DIR}"/*.yaml; do
  echo "  $(basename "${f}")"
  kubectl apply --server-side --force-conflicts -f "${f}"
done

echo ""
echo "CRD installation finished."
