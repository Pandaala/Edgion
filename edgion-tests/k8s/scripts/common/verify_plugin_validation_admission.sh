#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
INVALID_DIR="${PLUGIN_VALIDATION_INVALID_DIR:-${PROJECT_ROOT}/edgion-tests/k8s/invalid/EdgionPlugins/PluginValidationStatus}"

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH" >&2
  exit 1
fi
if [ ! -d "${INVALID_DIR}" ]; then
  echo "PluginValidationStatus invalid fixture directory not found: ${INVALID_DIR}" >&2
  exit 1
fi

invalid_count=0
while IFS= read -r invalid_fixture; do
  [ -n "${invalid_fixture}" ] || continue
  invalid_count=$((invalid_count + 1))
  if output="$(kubectl apply --dry-run=server --filename "${invalid_fixture}" 2>&1)"; then
    echo "Kubernetes admission unexpectedly accepted ${invalid_fixture}" >&2
    exit 1
  fi
  if ! printf '%s\n' "${output}" \
    | grep -qE 'strict decoding error|must validate one and only one schema'; then
    echo "Kubernetes admission failed for an unexpected reason: ${invalid_fixture}" >&2
    printf '%s\n' "${output}" >&2
    exit 1
  fi
  echo "Kubernetes admission rejected malformed fixture: $(basename "${invalid_fixture}")"
done < <(find "${INVALID_DIR}" -maxdepth 1 -type f -name '*.yaml' | sort)

if [ "${invalid_count}" -ne 3 ]; then
  echo "Expected 3 malformed ConditionSet fixtures, found ${invalid_count}" >&2
  exit 1
fi

echo "PluginValidationStatus admission preflight passed"
