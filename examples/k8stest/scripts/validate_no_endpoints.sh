#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONF_ROOT="${1:-$PROJECT_ROOT/examples/k8stest/conf}"

if [[ ! -d "$CONF_ROOT" ]]; then
  echo "[ERROR] k8s conf directory not found: $CONF_ROOT"
  exit 1
fi

echo "[INFO] validating no Endpoint/EndpointSlice in: $CONF_ROOT"

# 1) Strict kind check in YAML docs.
if matches=$(rg -n "^[[:space:]]*kind:[[:space:]]*Endpoint(Slice)?[[:space:]]*$" "$CONF_ROOT" -S); then
  echo "[ERROR] Endpoint/EndpointSlice kind found in k8s conf:"
  echo "$matches"
  exit 1
fi

# 2) File name guard to catch accidental resource copy.
# Only match Kubernetes resource-like names containing capitalized Endpoint/EndpointSlice.
if files=$(find "$CONF_ROOT" -type f \( -name '*.yaml' -o -name '*.yml' \) | grep -E '/[^/]*Endpoint(Slice)?[^/]*\.ya?ml$' || true); then
  if [[ -n "${files}" ]]; then
    echo "[ERROR] Endpoint/EndpointSlice-like file names found in k8s conf:"
    echo "$files"
    exit 1
  fi
fi

echo "[INFO] validation passed: no Endpoint/EndpointSlice resources found."
