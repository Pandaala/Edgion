#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/../edgion-deploy/kubernetes}"
TARGET_SCRIPT="$K8S_DEPLOY_ROOT/scripts/cleanup.sh"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/examples/k8stest/conf}"
SCRIPT_ARGS=()

WITH_IMAGES=false
NS_TIMEOUT_SECONDS=300
NAMESPACES=("edgion-system" "edgion-default" "edgion-test" "edgion-backend")

show_help() {
  cat <<EOF
Usage: $0 [options]

Options:
  --with-crds                    Also delete CRDs (passed through to deploy cleanup)
  --with-images                  Also clean old local test images
  --ns-timeout <seconds>         Wait timeout for namespace deletion (default: ${NS_TIMEOUT_SECONDS})
  -h, --help                     Show this help

Behavior:
  1) Delete all manifests under examples/k8stest/conf (cluster-scoped + namespaced)
  2) Run deploy cleanup script (workloads + namespaces [+ optional CRDs])
  3) Wait namespaces fully deleted
  4) Optional: clean old Edgion test images
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --with-crds)
      SCRIPT_ARGS+=("$1")
      shift
      ;;
    --with-images)
      WITH_IMAGES=true
      shift
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

if [[ ! -x "$TARGET_SCRIPT" ]]; then
  echo "cleanup script not found or not executable: $TARGET_SCRIPT"
  echo "set K8S_DEPLOY_ROOT to your deploy repo kubernetes directory"
  exit 1
fi

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found"
  exit 1
fi

delete_k8stest_conf_resources() {
  if [[ ! -d "${CONF_ROOT}" ]]; then
    echo "[skip] k8stest conf dir not found: ${CONF_ROOT}"
    return 0
  fi

  echo "[cleanup] delete resources from ${CONF_ROOT}"
  local deleted_any=false
  while IFS= read -r -d '' f; do
    deleted_any=true
    kubectl delete -f "${f}" --ignore-not-found=true >/dev/null 2>&1 || true
  done < <(find "${CONF_ROOT}" -type f \( -name '*.yaml' -o -name '*.yml' \) -print0)

  if [[ "${deleted_any}" == "false" ]]; then
    echo "[cleanup] no yaml found under ${CONF_ROOT}"
  fi
}

delete_generated_artifacts() {
  local generated_dir="${PROJECT_ROOT}/examples/k8stest/generated"
  if [[ -d "${generated_dir}" ]]; then
    echo "[cleanup] remove generated artifacts: ${generated_dir}"
    rm -rf "${generated_dir}"
  fi
}

wait_namespace_gone() {
  local ns="$1"
  local timeout="$2"
  local start
  start="$(date +%s)"

  while kubectl get namespace "${ns}" >/dev/null 2>&1; do
    if (( $(date +%s) - start > timeout )); then
      echo "[warn] namespace ${ns} still exists after ${timeout}s"
      kubectl get namespace "${ns}" -o wide || true
      return 1
    fi
    sleep 2
  done
  echo "[ok] namespace ${ns} deleted"
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

delete_k8stest_conf_resources
delete_generated_artifacts

echo "[cleanup] run deploy cleanup: ${TARGET_SCRIPT} ${SCRIPT_ARGS[*]:-}"
if ((${#SCRIPT_ARGS[@]} > 0)); then
  "${TARGET_SCRIPT}" "${SCRIPT_ARGS[@]}"
else
  "${TARGET_SCRIPT}"
fi

echo "[cleanup] wait namespaces deleted"
for ns in "${NAMESPACES[@]}"; do
  wait_namespace_gone "${ns}" "${NS_TIMEOUT_SECONDS}" || true
done

if [[ "${WITH_IMAGES}" == "true" ]]; then
  cleanup_old_test_images
fi

echo "Cleanup completed."
