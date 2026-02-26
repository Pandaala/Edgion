#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/examples/k8stest/conf}"
SUITE="${1:-}"

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH"
  exit 1
fi

if [[ ! -d "${CONF_ROOT}" ]]; then
  echo "k8s conf root not found: ${CONF_ROOT}"
  exit 1
fi

if [[ -z "${SUITE}" ]]; then
  echo "Usage: $0 <suite-dir>"
  echo "Example: $0 HTTPRoute/Match"
  exit 1
fi

SUITE_DIR="${CONF_ROOT}/${SUITE}"
if [[ ! -d "${SUITE_DIR}" ]]; then
  echo "suite directory not found: ${SUITE_DIR}"
  exit 1
fi

TMP_DIR="$(mktemp -d /tmp/edgion-k8s-apply.XXXXXX)"
DIRS_FILE="${TMP_DIR}/dirs.txt"
FILES_FILE="${TMP_DIR}/files.txt"
cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

echo "${CONF_ROOT}/base" > "${DIRS_FILE}"
echo "${SUITE_DIR}" >> "${DIRS_FILE}"

RESOURCE="${SUITE%%/*}"
RESOURCE_BASE="${CONF_ROOT}/${RESOURCE}/base"
if [[ -d "${RESOURCE_BASE}" ]]; then
  echo "${RESOURCE_BASE}" >> "${DIRS_FILE}"
fi

if [[ "${SUITE}" == HTTPRoute/Filters/* ]]; then
  FILTERS_BASE="${CONF_ROOT}/HTTPRoute/Filters/base"
  if [[ -d "${FILTERS_BASE}" ]]; then
    echo "${FILTERS_BASE}" >> "${DIRS_FILE}"
  fi
fi

if [[ "${SUITE}" == EdgionPlugins/* ]]; then
  PLUGINS_BASE="${CONF_ROOT}/EdgionPlugins/base"
  if [[ -d "${PLUGINS_BASE}" ]]; then
    echo "${PLUGINS_BASE}" >> "${DIRS_FILE}"
  fi
fi

sort -u "${DIRS_FILE}" -o "${DIRS_FILE}"

: > "${FILES_FILE}"
while IFS= read -r d; do
  [[ -d "${d}" ]] || continue
  find "${d}" -type f \( -name "*.yaml" -o -name "*.yml" \) | sort >> "${FILES_FILE}"
done < "${DIRS_FILE}"
sort -u "${FILES_FILE}" -o "${FILES_FILE}"

if [[ ! -s "${FILES_FILE}" ]]; then
  echo "no YAML files found for suite ${SUITE}"
  exit 1
fi

while IFS= read -r f; do
  [[ -n "${f}" ]] || continue
  echo "Applying ${f}"
  ok=false
  for attempt in 1 2 3; do
    out="$(kubectl apply -f "${f}" 2>&1)" && rc=0 || rc=$?
    if [[ "${rc}" -eq 0 ]]; then
      echo "${out}"
      ok=true
      break
    fi

    echo "${out}"
    # Some resources are not always apply-friendly on repeated runs.
    if echo "${out}" | grep -qE 'must be specified for an update|Operation cannot be fulfilled|strict decoding error|Invalid value:'; then
      echo "Apply fallback to recreate: ${f}"
      kubectl delete -f "${f}" --ignore-not-found=true >/dev/null 2>&1 || true
      if kubectl create -f "${f}"; then
        ok=true
        break
      fi
    fi

    echo "Apply failed (attempt ${attempt}/3): ${f}"
    sleep 1
  done

  if [[ "${ok}" != "true" ]]; then
    echo "failed to apply ${f} after retries"
    exit 1
  fi
done < "${FILES_FILE}"

echo "Applied suite: ${SUITE}"
