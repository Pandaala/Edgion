#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/kubernetes}"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/conf}"
GENERATED_DIR="$PROJECT_ROOT/edgion-tests/k8s/generated"

WITH_IMAGES=false
SKIP_GENERATED_ARTIFACTS=false
OWNERSHIP_FILE=""
GENERATED_OWNERSHIP_FILE=""
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
  "dragonfly-system"
  "edgion-backend"
  "edgion-default"
  "edgion-deleg"
  "edgion-deleg-dyn"
  "edgion-grpc-deleg"
  "edgion-grpc-deleg-dyn"
  "edgion-it"
  "edgion-system"
  "edgion-test"
  "other"
)

show_help() {
  cat <<EOF_HELP
Usage: $0 [options]

Options:
  --with-images                  Also clean old local test images
  --skip-generated-artifacts    Preserve generated test artifacts
  --ownership-file <path>       Use the run's exact namespace/cluster ownership inventory
  --ns-timeout <seconds>         Wait timeout for namespace deletion (default: ${NS_TIMEOUT_SECONDS})
  -h, --help                     Show this help

Behavior:
  1) Delete exact cluster-scoped objects from the ownership inventory
  2) Delete all integration namespaces in one bounded operation
  3) Wait for cluster-scoped objects to be fully deleted
  4) Wait for namespaces to be fully deleted
  5) Optional: clean old Edgion test images

CRDs are installation-scoped and are never deleted by this integration cleanup.
EOF_HELP
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --with-images)
      WITH_IMAGES=true
      shift
      ;;
    --skip-generated-artifacts)
      SKIP_GENERATED_ARTIFACTS=true
      shift
      ;;
    --ownership-file)
      OWNERSHIP_FILE="${2:-}"
      if [[ -z "${OWNERSHIP_FILE}" || ! -f "${OWNERSHIP_FILE}" ]]; then
        echo "invalid --ownership-file: ${OWNERSHIP_FILE:-<empty>}"
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

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found"
  exit 1
fi

if [[ ! -f "$K8S_DEPLOY_ROOT/namespace.yaml" ]]; then
  echo "namespace manifest not found: $K8S_DEPLOY_ROOT/namespace.yaml"
  exit 1
fi

write_cleanup_inventory() {
  python3 - "${CONF_ROOT}" "${K8S_DEPLOY_ROOT}" <<'PY'
import pathlib
import sys

import yaml

cluster_kinds = {"ClusterRole", "ClusterRoleBinding", "EdgionGatewayConfig", "GatewayClass"}
keys = set()
for raw_root in sys.argv[1:]:
    root = pathlib.Path(raw_root)
    if not root.exists():
        continue
    for path in sorted(set(root.rglob("*.yaml")) | set(root.rglob("*.yml"))):
        if path.name.endswith(".template.yaml"):
            continue
        for document in yaml.safe_load_all(path.read_text()):
            if not isinstance(document, dict):
                continue
            api_version = document.get("apiVersion")
            kind = document.get("kind")
            metadata = document.get("metadata") or {}
            name = metadata.get("name")
            namespace = metadata.get("namespace")
            group = api_version.split("/", 1)[0] if isinstance(api_version, str) and "/" in api_version else ""
            locator = f"{kind}.{group}" if group else kind
            if namespace:
                keys.add(("namespace", "Namespace", "Namespace", namespace))
            if kind == "Namespace" and name:
                keys.add(("namespace", "Namespace", "Namespace", name))
            elif kind in cluster_kinds and name:
                keys.add(("cluster", kind, locator, name))

for key in sorted(keys):
    print("\t".join(key))
PY
}

prepare_ownership_inventory() {
  if [[ -n "${OWNERSHIP_FILE}" ]]; then
    return 0
  fi
  GENERATED_OWNERSHIP_FILE="$(mktemp /tmp/edgion-k8s-cleanup-ownership.XXXXXX)"
  write_cleanup_inventory > "${GENERATED_OWNERSHIP_FILE}"
  OWNERSHIP_FILE="${GENERATED_OWNERSHIP_FILE}"
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
      'ClusterRole|ClusterRole.rbac.authorization.k8s.io' \
      |'ClusterRoleBinding|ClusterRoleBinding.rbac.authorization.k8s.io' \
      |'EdgionGatewayConfig|EdgionGatewayConfig.edgion.io' \
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
    if ! kubectl delete "${OWNED_CLUSTER_LOCATORS[${index}]}" "${OWNED_CLUSTER_NAMES[${index}]}" \
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
    if ! kubectl delete namespace "${CLEANUP_NAMESPACES[@]}" \
      --ignore-not-found=true --wait=false >/dev/null; then
      echo "failed to request integration namespace deletion" >&2
      failed=1
    fi
  fi

  return "${failed}"
}

delete_generated_artifacts() {
  if [[ "${SKIP_GENERATED_ARTIFACTS}" == "true" ]]; then
    echo "[cleanup] preserve generated artifacts"
    return 0
  fi
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
    output="$(kubectl get namespace "${ns}" --ignore-not-found -o name 2>&1)" && rc=0 || rc=$?
    if [[ "${rc}" -eq 0 && -z "${output}" ]]; then
      echo "[ok] namespace ${ns} deleted"
      return 0
    fi
    if (( $(date +%s) - start > timeout )); then
      if [[ "${rc}" -ne 0 ]]; then
        echo "[warn] namespace ${ns} deletion could not be verified after ${timeout}s: ${output}"
      else
        echo "[warn] namespace ${ns} still exists after ${timeout}s"
        kubectl get namespace "${ns}" -o wide || true
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
    output="$(kubectl get "${locator}" "${name}" --ignore-not-found -o name 2>&1)" && rc=0 || rc=$?
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

cleanup_old_test_images() {
  if ! command -v docker >/dev/null 2>&1; then
    echo "[skip] docker not found, skip image cleanup"
    return 0
  fi

  echo "[cleanup] remove dangling images"
  docker image prune -f >/dev/null 2>&1 || true

  echo "[cleanup] remove old edgion test images (keep latest per repository)"

  local container_ids
  local used_short_ids=""
  container_ids="$(docker ps -aq || true)"
  if [[ -n "${container_ids}" ]]; then
    used_short_ids="$(docker inspect -f '{{.Image}}' ${container_ids} 2>/dev/null \
      | sed 's/^sha256://g' \
      | cut -c1-12 \
      | sort -u || true)"
  fi

  local repos
  repos="$(docker images --format '{{.Repository}}' \
    | awk -F/ '
      {
        b=$NF
        if (b ~ /^edgion-(controller|gateway|test-server|test-client)$/ || b=="edgion-builder") {
          print $0
        }
      }' \
    | sort -u)"

  if [[ -z "${repos}" ]]; then
    echo "[cleanup] no matching edgion test repos"
    return 0
  fi

  local repo latest_id id
  while IFS= read -r repo; do
    [[ -z "${repo}" ]] && continue
    latest_id="$(docker images "${repo}" --format '{{.ID}}' | awk 'NR==1{print;exit}')"
    [[ -z "${latest_id}" ]] && continue

    while IFS= read -r id; do
      [[ -z "${id}" ]] && continue
      if [[ "${id}" == "${latest_id}" ]]; then
        continue
      fi
      if [[ -n "${used_short_ids}" ]] && grep -Fxq "${id}" <<< "${used_short_ids}"; then
        echo "[skip] image in use: ${id} (${repo})"
        continue
      fi
      docker rmi -f "${id}" >/dev/null 2>&1 || true
    done < <(docker images "${repo}" --format '{{.ID}}' | awk '!seen[$0]++')
  done <<< "${repos}"

  echo "[ok] image cleanup done"
}

prepare_ownership_inventory
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

if [[ "${WITH_IMAGES}" == "true" ]]; then
  cleanup_old_test_images
fi

if [[ -n "${GENERATED_OWNERSHIP_FILE}" ]]; then
  rm -f "${GENERATED_OWNERSHIP_FILE}"
fi

echo "Cleanup completed."
exit "${cleanup_failed}"
