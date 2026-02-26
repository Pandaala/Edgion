#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONF_ROOT="${1:-$PROJECT_ROOT/examples/k8stest/conf}"

if [[ ! -d "$CONF_ROOT" ]]; then
  echo "[ERROR] k8s conf directory not found: $CONF_ROOT"
  exit 1
fi

echo "[INFO] validating no Endpoint/EndpointSlice in: $CONF_ROOT"

# 1) Strict kind check in YAML docs.
KIND_PATTERN="^[[:space:]]*kind:[[:space:]]*Endpoint(Slice)?[[:space:]]*$"
if command -v rg >/dev/null 2>&1; then
  matches="$(rg -n "$KIND_PATTERN" "$CONF_ROOT" -S || true)"
else
  matches="$(find "$CONF_ROOT" -type f \( -name '*.yaml' -o -name '*.yml' \) -print0 | xargs -0 grep -nE "$KIND_PATTERN" || true)"
fi
if [[ -n "${matches}" ]]; then
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
