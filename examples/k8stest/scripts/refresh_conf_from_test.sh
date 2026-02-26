#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
GENERATE_SCRIPT="$PROJECT_ROOT/examples/test/scripts/utils/generate_k8s_conf.sh"
VALIDATE_SCRIPT="$SCRIPT_DIR/validate_no_endpoints.sh"
OUT_DIR="${1:-$PROJECT_ROOT/examples/k8stest/conf}"

if [[ ! -x "$GENERATE_SCRIPT" ]]; then
  echo "generate script not found or not executable: $GENERATE_SCRIPT"
  exit 1
fi

if [[ ! -x "$VALIDATE_SCRIPT" ]]; then
  echo "validate script not found or not executable: $VALIDATE_SCRIPT"
  exit 1
fi

echo "[1/2] Generate k8s conf from examples/test/conf -> ${OUT_DIR}"
"$GENERATE_SCRIPT" "$OUT_DIR"

echo "[2/2] Validate no Endpoint/EndpointSlice"
"$VALIDATE_SCRIPT" "$OUT_DIR"

echo "Refresh finished: ${OUT_DIR}"
