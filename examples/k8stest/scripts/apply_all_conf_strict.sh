#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/examples/k8stest/conf}"

show_help() {
  cat <<EOF
Usage: $0 [options] [conf-root]

Apply all YAML resources under k8s conf root in sorted order.
Strict mode: any apply error exits immediately.

Options:
  --include-bootstrap   Include 00-namespace.yaml and 01-deployment.yaml
  --include-dynamic-updates Include Gateway/DynamicTest/updates and delete resources
  -h, --help            Show this help
EOF
}

INCLUDE_BOOTSTRAP=false
INCLUDE_DYNAMIC_UPDATES=false
ARGS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --include-bootstrap)
      INCLUDE_BOOTSTRAP=true
      shift
      ;;
    --include-dynamic-updates)
      INCLUDE_DYNAMIC_UPDATES=true
      shift
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      ARGS+=("$1")
      shift
      ;;
  esac
done

if [[ ${#ARGS[@]} -gt 0 ]]; then
  CONF_ROOT="${ARGS[0]}"
fi

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH"
  exit 1
fi

if [[ ! -d "${CONF_ROOT}" ]]; then
  echo "k8s conf root not found: ${CONF_ROOT}"
  exit 1
fi

FILES_FILE="$(mktemp /tmp/edgion-k8s-apply-all.XXXXXX)"
cleanup() {
  rm -f "${FILES_FILE}"
}
trap cleanup EXIT

find "${CONF_ROOT}" -type f \( -name "*.yaml" -o -name "*.yml" \) | sort > "${FILES_FILE}"
if [[ ! -s "${FILES_FILE}" ]]; then
  echo "no YAML files found under: ${CONF_ROOT}"
  exit 1
fi

if [[ "${INCLUDE_BOOTSTRAP}" != "true" ]]; then
  grep -Ev '/(00-namespace\.ya?ml|01-deployment\.ya?ml)$' "${FILES_FILE}" > "${FILES_FILE}.filtered"
  mv "${FILES_FILE}.filtered" "${FILES_FILE}"
fi

if [[ "${INCLUDE_DYNAMIC_UPDATES}" != "true" ]]; then
  grep -Ev '/Gateway/DynamicTest/(updates|delete)/' "${FILES_FILE}" > "${FILES_FILE}.filtered"
  mv "${FILES_FILE}.filtered" "${FILES_FILE}"
fi

# TLS and mTLS secrets are generated at runtime by run_k8s_integration.sh.
grep -Ev '/(base/Secret_edgion-test_edge-tls|EdgionTls/mTLS/Secret_edge_client-ca|EdgionTls/mTLS/Secret_edge_ca-chain|EdgionTls/mTLS/Secret_edge_mtls-server|HTTPRoute/Backend/BackendTLS/Secret_backend-ca)\.ya?ml$' "${FILES_FILE}" > "${FILES_FILE}.filtered"
mv "${FILES_FILE}.filtered" "${FILES_FILE}"

# K8s admission rejects this manifest before controller-level conflict logic can observe it.
# Keep it for documentation/testing reference, but do not include it in bulk strict apply.
grep -Ev '/Gateway/PortConflict/Gateway_internal_conflict\.ya?ml$' "${FILES_FILE}" > "${FILES_FILE}.filtered"
mv "${FILES_FILE}.filtered" "${FILES_FILE}"

count="$(wc -l < "${FILES_FILE}" | tr -d ' ')"
echo "Applying ${count} resources from ${CONF_ROOT} (strict mode, include_bootstrap=${INCLUDE_BOOTSTRAP}, include_dynamic_updates=${INCLUDE_DYNAMIC_UPDATES})"

while IFS= read -r f; do
  [[ -n "${f}" ]] || continue
  echo "Applying ${f}"
  ok=false
  saw_conflict=false
  for attempt in 1 2 3; do
    out="$(kubectl apply --server-side --force-conflicts --field-manager=edgion-k8s-test -f "${f}" 2>&1)" && rc=0 || rc=$?
    if [[ "${rc}" -eq 0 ]]; then
      echo "${out}"
      ok=true
      break
    fi

    # Retry only on optimistic locking conflicts.
    if echo "${out}" | grep -qE 'Operation cannot be fulfilled|the object has been modified'; then
      saw_conflict=true
      echo "Apply conflict (attempt ${attempt}/3), retrying: ${f}"
      sleep 1
      continue
    fi

    echo "${out}"
    exit "${rc}"
  done

  if [[ "${ok}" != "true" && "${saw_conflict}" == "true" ]]; then
    echo "Apply conflict persists, fallback to replace --force: ${f}"
    if kubectl replace --force -f "${f}"; then
      ok=true
    fi
  fi

  if [[ "${ok}" != "true" ]]; then
    echo "failed to apply ${f} after conflict retries"
    exit 1
  fi
done < "${FILES_FILE}"

echo "Strict apply finished successfully."
