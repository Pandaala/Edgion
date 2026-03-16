#!/usr/bin/env bash
# Remove Edgion Gateway from a Kubernetes cluster.
#
# Usage:
#   ./cleanup.sh [OPTIONS]
#
# Options:
#   --with-crds          Also delete all CRDs (Gateway API + Edgion)
#   --with-namespace     Also delete the edgion-system namespace (destructive!)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

WITH_CRDS=false
WITH_NAMESPACE=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) sed -n '/^# /s/^# //p' "$0"; exit 0 ;;
    --with-crds) WITH_CRDS=true; shift ;;
    --with-namespace) WITH_NAMESPACE=true; shift ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

command -v kubectl >/dev/null 2>&1 || { echo "kubectl not found" >&2; exit 1; }

echo "[1] Delete base config"
kubectl delete -f "${ROOT_DIR}/base-config/" --ignore-not-found=true || true

echo "[2] Delete gateway"
kubectl delete -f "${ROOT_DIR}/gateway/" --ignore-not-found=true || true

echo "[3] Delete controller"
kubectl delete -f "${ROOT_DIR}/controller/" --ignore-not-found=true || true

if [[ "${WITH_NAMESPACE}" == true ]]; then
  echo "[4] Delete namespace"
  kubectl delete namespace edgion-system --ignore-not-found=true || true
else
  echo "[4] Skip namespace deletion (pass --with-namespace to delete)"
fi

if [[ "${WITH_CRDS}" == true ]]; then
  echo "[5] Delete CRDs"
  "${SCRIPT_DIR}/install_crds.sh" --delete
else
  echo "[5] Skip CRD deletion (pass --with-crds to delete)"
fi

echo ""
echo "Cleanup finished."
