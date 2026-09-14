#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/kubernetes}"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/conf}"
OWNERSHIP_INVENTORY_SCRIPT="$SCRIPT_DIR/common/write_cleanup_inventory.sh"

OWNERSHIP_FILE=""
EXPECTED_CONTEXT=""
RUN_DIR=""
GENERATED_DIR=""
CLEANUP_NAMESPACES=()
OWNED_CLUSTER_LOCATORS=()
OWNED_CLUSTER_NAMES=()
CLEANUP_NAMESPACE_COUNT=0
OWNED_CLUSTER_COUNT=0
NS_TIMEOUT_SECONDS=300
NAMESPACES=(
  "app"
  "app2"
  "backend"
  "cert-store"
  "edgion-backend"
  "edgion-default"
  "edgion-deleg"
  "edgion-deleg-dyn"
  "edgion-grpc-deleg"
  "edgion-grpc-deleg-dyn"
  "edgion-it"
  "edgion-test"
  "other"
)

show_help() {
  cat <<EOF_HELP
Usage: $0 [options]

Options:
  --ownership-file <path>       Use the run's exact namespace/cluster ownership inventory
  --expected-context <name>     Require and pin every kubectl call to this context
  --ns-timeout <seconds>         Wait timeout for namespace deletion (default: ${NS_TIMEOUT_SECONDS})
  -h, --help                     Show this help

Behavior:
  1) Delete exact cluster-scoped objects from the ownership inventory
  2) Delete run-owned test namespaces in one bounded operation
  3) Wait for cluster-scoped objects to be fully deleted
  4) Wait for namespaces to be fully deleted
  5) Remove only the generated artifacts beside the ownership inventory

CRDs, installation namespaces/RBAC, and local images are never deleted by this cleanup.
EOF_HELP
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ownership-file)
      OWNERSHIP_FILE="${2:-}"
      if [[ -z "${OWNERSHIP_FILE}" || ! -f "${OWNERSHIP_FILE}" ]]; then
        echo "invalid --ownership-file: ${OWNERSHIP_FILE:-<empty>}"
        exit 1
      fi
      shift 2
      ;;
    --expected-context)
      EXPECTED_CONTEXT="${2:-}"
      if [[ -z "${EXPECTED_CONTEXT}" ]]; then
        echo "invalid --expected-context: <empty>"
        exit 1
      fi
      shift 2
      ;;
    --ns-timeout)
      NS_TIMEOUT_SECONDS="${2:-}"
      if ! [[ "${NS_TIMEOUT_SECONDS}" =~ ^[0-9]+$ ]]; then
        echo "invalid --ns-timeout: ${NS_TIMEOUT_SECONDS}"
        exit 1
      fi
      shift 2
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      echo "unknown argument: $1"
      show_help
      exit 1
      ;;
  esac
done

if [[ -z "${OWNERSHIP_FILE}" ]]; then
  echo "--ownership-file is required; cleanup never infers ownership from fixtures" >&2
  exit 1
fi
if [[ "$(basename "${OWNERSHIP_FILE}")" != "owned-cluster-keys.tsv" ]]; then
  echo "ownership file must be named owned-cluster-keys.tsv" >&2
  exit 1
fi
if [[ -z "${EXPECTED_CONTEXT}" ]]; then
  echo "--expected-context is required" >&2
  exit 1
fi

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found"
  exit 1
fi
if [[ ! -x "${OWNERSHIP_INVENTORY_SCRIPT}" ]]; then
  echo "ownership inventory generator not found: ${OWNERSHIP_INVENTORY_SCRIPT}" >&2
  exit 1
fi

current_context="$(kubectl config current-context 2>&1)" || {
  echo "failed to read the current kubectl context: ${current_context}" >&2
  exit 1
}
if [[ "${current_context}" != "${EXPECTED_CONTEXT}" ]]; then
  echo "current kubectl context ${current_context} does not match --expected-context ${EXPECTED_CONTEXT}" >&2
  exit 1
fi
KUBECTL=(kubectl --context "${EXPECTED_CONTEXT}")

RUN_DIR="$(cd "$(dirname "${OWNERSHIP_FILE}")" && pwd)"
if [[ "${RUN_DIR}" == "/" ]]; then
  echo "refusing to use the filesystem root as a cleanup run directory" >&2
  exit 1
fi
GENERATED_DIR="${RUN_DIR}/generated"

if [[ ! -f "$K8S_DEPLOY_ROOT/namespace.yaml" ]]; then
  echo "namespace manifest not found: $K8S_DEPLOY_ROOT/namespace.yaml"
  exit 1
fi

write_cleanup_inventory() {
  "${OWNERSHIP_INVENTORY_SCRIPT}" "${CONF_ROOT}" "${K8S_DEPLOY_ROOT}"
}

is_allowed_namespace() {
  local candidate="$1" allowed
  for allowed in "${NAMESPACES[@]}"; do
    [[ "${candidate}" == "${allowed}" ]] && return 0
  done
  return 1
}

validate_ownership_inventory() {
  local scope kind locator name extra
  local existing index duplicate entry
  local canonical_inventory
  canonical_inventory="$(write_cleanup_inventory)"

  while IFS=$'\t' read -r scope kind locator name extra; do
    if [[ -z "${scope}${kind}${locator}${name}${extra}" ]]; then
      continue
    fi
    if [[ -n "${extra}" || -z "${scope}" || -z "${kind}" || -z "${locator}" || -z "${name}" ]]; then
      echo "invalid ownership inventory row: expected exactly four non-empty tab-separated fields" >&2
      return 1
    fi
    if [[ ! "${name}" =~ ^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$ ]]; then
      echo "invalid ownership name: ${name}" >&2
      return 1
    fi
    entry="${scope}"$'\t'"${kind}"$'\t'"${locator}"$'\t'"${name}"
    if ! grep -Fqx -- "${entry}" <<< "${canonical_inventory}"; then
      echo "ownership entry is not present in the current integration manifests: ${kind}/${name}" >&2
      return 1
    fi

    if [[ "${scope}" == "namespace" ]]; then
      if [[ "${kind}|${locator}" != "Namespace|Namespace" ]] || ! is_allowed_namespace "${name}"; then
        echo "unapproved namespace ownership entry: ${kind}/${locator}/${name}" >&2
        return 1
      fi
      duplicate=false
      for ((index = 0; index < CLEANUP_NAMESPACE_COUNT; index++)); do
        existing="${CLEANUP_NAMESPACES[${index}]}"
        [[ "${existing}" == "${name}" ]] && duplicate=true
      done
      if [[ "${duplicate}" == "false" ]]; then
        CLEANUP_NAMESPACES[${CLEANUP_NAMESPACE_COUNT}]="${name}"
        CLEANUP_NAMESPACE_COUNT=$((CLEANUP_NAMESPACE_COUNT + 1))
      fi
      continue
    fi

    if [[ "${scope}" != "cluster" ]]; then
      echo "unapproved ownership scope: ${scope}" >&2
      return 1
    fi
    case "${kind}|${locator}" in
      'EdgionGatewayConfig|EdgionGatewayConfig.edgion.io' \
      |'GatewayClass|GatewayClass.gateway.networking.k8s.io') ;;
      *)
        echo "unapproved cluster-scoped ownership entry: ${kind}/${locator}/${name}" >&2
        return 1
        ;;
    esac
    duplicate=false
    for ((index = 0; index < OWNED_CLUSTER_COUNT; index++)); do
      if [[ "${OWNED_CLUSTER_LOCATORS[${index}]}" == "${locator}" \
        && "${OWNED_CLUSTER_NAMES[${index}]}" == "${name}" ]]; then
        duplicate=true
      fi
    done
    if [[ "${duplicate}" == "false" ]]; then
      OWNED_CLUSTER_LOCATORS[${OWNED_CLUSTER_COUNT}]="${locator}"
      OWNED_CLUSTER_NAMES[${OWNED_CLUSTER_COUNT}]="${name}"
      OWNED_CLUSTER_COUNT=$((OWNED_CLUSTER_COUNT + 1))
    fi
  done < "${OWNERSHIP_FILE}"
}

delete_owned_cluster_resources() {
  local index failed=0
  echo "[cleanup] delete exact cluster-scoped integration resources"
  for ((index = 0; index < OWNED_CLUSTER_COUNT; index++)); do
    if ! "${KUBECTL[@]}" delete "${OWNED_CLUSTER_LOCATORS[${index}]}" "${OWNED_CLUSTER_NAMES[${index}]}" \
      --ignore-not-found=true --wait=false >/dev/null; then
      echo "failed to delete ${OWNED_CLUSTER_LOCATORS[${index}]}/${OWNED_CLUSTER_NAMES[${index}]}" >&2
      failed=1
    fi
  done
  return "${failed}"
}

delete_integration_namespaces() {
  local failed=0
  if [[ "${CLEANUP_NAMESPACE_COUNT}" -gt 0 ]]; then
    echo "[cleanup] delete ${CLEANUP_NAMESPACE_COUNT} integration namespaces"
    if ! "${KUBECTL[@]}" delete namespace "${CLEANUP_NAMESPACES[@]}" \
      --ignore-not-found=true --wait=false >/dev/null; then
      echo "failed to request integration namespace deletion" >&2
      failed=1
    fi
  fi

  return "${failed}"
}

delete_generated_artifacts() {
  if [[ -d "${GENERATED_DIR}" ]]; then
    echo "[cleanup] remove generated artifacts: ${GENERATED_DIR}"
    rm -rf "${GENERATED_DIR}"
  fi
}

wait_namespace_gone() {
  local ns="$1"
  local timeout="$2"
  local start
  start="$(date +%s)"

  local output rc
  while true; do
    output="$("${KUBECTL[@]}" get namespace "${ns}" --ignore-not-found -o name 2>&1)" && rc=0 || rc=$?
    if [[ "${rc}" -eq 0 && -z "${output}" ]]; then
      echo "[ok] namespace ${ns} deleted"
      return 0
    fi
    if (( $(date +%s) - start > timeout )); then
      if [[ "${rc}" -ne 0 ]]; then
        echo "[warn] namespace ${ns} deletion could not be verified after ${timeout}s: ${output}"
      else
        echo "[warn] namespace ${ns} still exists after ${timeout}s"
        "${KUBECTL[@]}" get namespace "${ns}" -o wide || true
      fi
      return 1
    fi
    sleep 2
  done
}

wait_cluster_resource_gone() {
  local locator="$1" name="$2" timeout="$3"
  local start output rc
  start="$(date +%s)"
  while true; do
    output="$("${KUBECTL[@]}" get "${locator}" "${name}" --ignore-not-found -o name 2>&1)" && rc=0 || rc=$?
    if [[ "${rc}" -eq 0 && -z "${output}" ]]; then
      echo "[ok] ${locator}/${name} deleted"
      return 0
    fi
    if (( $(date +%s) - start > timeout )); then
      if [[ "${rc}" -ne 0 ]]; then
        echo "${locator}/${name} deletion could not be verified after ${timeout}s: ${output}" >&2
      else
        echo "${locator}/${name} still exists after ${timeout}s" >&2
      fi
      return 1
    fi
    sleep 2
  done
}

validate_ownership_inventory
delete_generated_artifacts
cleanup_failed=0
delete_owned_cluster_resources || cleanup_failed=1
delete_integration_namespaces || cleanup_failed=1

echo "[cleanup] wait cluster-scoped resources deleted"
for ((index = 0; index < OWNED_CLUSTER_COUNT; index++)); do
  wait_cluster_resource_gone "${OWNED_CLUSTER_LOCATORS[${index}]}" \
    "${OWNED_CLUSTER_NAMES[${index}]}" "${NS_TIMEOUT_SECONDS}" || cleanup_failed=1
done

echo "[cleanup] wait namespaces deleted"
for ((index = 0; index < CLEANUP_NAMESPACE_COUNT; index++)); do
  wait_namespace_gone "${CLEANUP_NAMESPACES[${index}]}" "${NS_TIMEOUT_SECONDS}" || cleanup_failed=1
done

echo "Cleanup completed."
exit "${cleanup_failed}"
