#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-${PROJECT_ROOT}/edgion-tests/k8s/conf}"
PORTS_CONFIG="${PORTS_CONFIG:-${PROJECT_ROOT}/edgion-tests/integration/conf/ports.json}"
DYNAMIC_MANIFEST="${DYNAMIC_MANIFEST:-${SCRIPT_DIR}/k8s_dynamic_scenarios.tsv}"
ENDPOINT_VALIDATOR="${SCRIPT_DIR}/validate_no_endpoints.sh"
ENDPOINT_INVENTORY="${ENDPOINT_INVENTORY:-${SCRIPT_DIR}/endpoint_inventory.sh}"
TEST_SERVER_IMAGE="${TEST_SERVER_IMAGE:-}"
PULL_POLICY_OVERRIDE="${PULL_POLICY_OVERRIDE:-}"

NAMESPACE="${NAMESPACE:-edgion-test}"
POD="${POD:-}"
CONTAINER="${CONTAINER:-test-client}"
TARGET_HOST="${TARGET_HOST:-edgion-gateway.edgion-system.svc.cluster.local}"
TEST_CLIENT_BIN_PATH="${TEST_CLIENT_BIN_PATH:-/usr/local/bin/test_client}"
CONTROLLER_ADMIN_URL="${EDGION_CONTROLLER_ADMIN_URL:-http://edgion-controller-leader.edgion-system.svc.cluster.local:12101}"
GATEWAY_ADMIN_URL="${EDGION_TEST_GATEWAY_ADMIN_URL:-http://edgion-gateway.edgion-system.svc.cluster.local:12001}"
CONTROLLER_ADMIN_TOKEN="${EDGION_TEST_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-integration-gateway-admin-token-0123456789}"
SYNC_TIMEOUT_SECONDS="${SYNC_TIMEOUT_SECONDS:-30}"
SYNC_POLL_INTERVAL_SECONDS="${SYNC_POLL_INTERVAL_SECONDS:-1}"
PHASE_EXEC_TIMEOUT_SECONDS="${PHASE_EXEC_TIMEOUT_SECONDS:-600}"

LIST_ONLY=false
REQUESTED_SCENARIOS=()

show_help() {
  cat <<EOF
Usage: $0 [options]

Run dynamic scenarios whose delta fixtures are owned by the Kubernetes conf root.
The K8s conf tree is validated and staged in an isolated test-client work directory;
local integration YAML fixtures are never staged or applied.

Options:
  --conf-root PATH   Kubernetes conf root (default: edgion-tests/k8s/conf)
  --scenario ID      Run one supported scenario; may be repeated
  --list             List supported Kubernetes dynamic scenarios
  -h, --help         Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --conf-root)
      if [[ $# -lt 2 ]]; then
        echo "ERROR: --conf-root requires a path" >&2
        exit 2
      fi
      CONF_ROOT="$2"
      shift 2
      ;;
    --scenario)
      if [[ $# -lt 2 ]]; then
        echo "ERROR: --scenario requires an id" >&2
        exit 2
      fi
      REQUESTED_SCENARIOS+=("$2")
      shift 2
      ;;
    --list)
      LIST_ONLY=true
      shift
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      echo "ERROR: unknown option: $1" >&2
      show_help >&2
      exit 2
      ;;
  esac
done

if [[ ! -d "${CONF_ROOT}" ]]; then
  echo "ERROR: K8s conf root not found: ${CONF_ROOT}" >&2
  exit 1
fi
if [[ ! -x "${ENDPOINT_VALIDATOR}" ]]; then
  echo "ERROR: Endpoint validator not found or not executable: ${ENDPOINT_VALIDATOR}" >&2
  exit 1
fi
if [[ ! -f "${DYNAMIC_MANIFEST}" ]]; then
  echo "ERROR: K8s dynamic scenario manifest not found: ${DYNAMIC_MANIFEST}" >&2
  exit 1
fi

# This guard runs before any kubectl call. It prevents local-style Endpoint or
# EndpointSlice fixtures from entering the staged scenario tree.
"${ENDPOINT_VALIDATOR}" "${CONF_ROOT}"

MANIFEST_IDS=()
MANIFEST_PATHS=()
MANIFEST_RESET_PATHS=()
MANIFEST_KUBECTL_RESOURCES=()
MANIFEST_GATEWAY_KINDS=()
MANIFEST_NAMESPACES=()
MANIFEST_NAMES=()
# A manifest namespace of "-" denotes a cluster-scoped primary resource.
while IFS=$'\t' read -r scenario_id conf_path reset_path kubectl_resource gateway_kind resource_namespace resource_name extra \
  || [[ -n "${scenario_id}" ]]; do
  scenario_id="${scenario_id%$'\r'}"
  conf_path="${conf_path%$'\r'}"
  reset_path="${reset_path%$'\r'}"
  kubectl_resource="${kubectl_resource%$'\r'}"
  gateway_kind="${gateway_kind%$'\r'}"
  resource_namespace="${resource_namespace%$'\r'}"
  resource_name="${resource_name%$'\r'}"
  extra="${extra%$'\r'}"
  [[ -z "${scenario_id}" || "${scenario_id}" == \#* ]] && continue
  if [[ -z "${conf_path}" || -z "${reset_path}" || -z "${kubectl_resource}" \
    || -z "${gateway_kind}" || -z "${resource_namespace}" || -z "${resource_name}" \
    || -n "${extra}" ]]; then
    echo "ERROR: invalid dynamic manifest row for ${scenario_id}" >&2
    exit 1
  fi
  case "${scenario_id}" in
    *[!A-Za-z0-9_]*|'')
      echo "ERROR: invalid dynamic scenario id in manifest: ${scenario_id}" >&2
      exit 1
      ;;
  esac
  case "/${conf_path}/${reset_path}/" in
    /*/../*|//*|/*/./*)
      echo "ERROR: dynamic manifest paths must be normalized and relative for ${scenario_id}" >&2
      exit 1
      ;;
  esac
  case "${kubectl_resource}:${gateway_kind}:${resource_namespace}:${resource_name}" in
    *[!A-Za-z0-9._:-]*)
      echo "ERROR: invalid resource identity in dynamic manifest for ${scenario_id}" >&2
      exit 1
      ;;
  esac
  for existing_id in ${MANIFEST_IDS[@]+"${MANIFEST_IDS[@]}"}; do
    if [[ "${existing_id}" == "${scenario_id}" ]]; then
      echo "ERROR: duplicate dynamic scenario id in manifest: ${scenario_id}" >&2
      exit 1
    fi
  done
  if [[ ! -d "${CONF_ROOT}/${conf_path}" ]]; then
    echo "ERROR: K8s dynamic scenario directory is missing: ${CONF_ROOT}/${conf_path}" >&2
    exit 1
  fi
  if [[ ! -d "${CONF_ROOT}/${conf_path}/${reset_path}" ]]; then
    echo "ERROR: K8s dynamic reset directory is missing: ${CONF_ROOT}/${conf_path}/${reset_path}" >&2
    exit 1
  fi
  MANIFEST_IDS+=("${scenario_id}")
  MANIFEST_PATHS+=("${conf_path}")
  MANIFEST_RESET_PATHS+=("${reset_path}")
  MANIFEST_KUBECTL_RESOURCES+=("${kubectl_resource}")
  MANIFEST_GATEWAY_KINDS+=("${gateway_kind}")
  MANIFEST_NAMESPACES+=("${resource_namespace}")
  MANIFEST_NAMES+=("${resource_name}")
done < "${DYNAMIC_MANIFEST}"

if [[ ${#MANIFEST_IDS[@]} -eq 0 ]]; then
  echo "ERROR: K8s dynamic scenario manifest is empty" >&2
  exit 1
fi

if [[ "${LIST_ONLY}" == "true" ]]; then
  for scenario_id in "${MANIFEST_IDS[@]}"; do
    echo "${scenario_id}"
  done
  exit 0
fi

SELECTED_SCENARIOS=()
if [[ ${#REQUESTED_SCENARIOS[@]} -eq 0 ]]; then
  SELECTED_SCENARIOS=("${MANIFEST_IDS[@]}")
else
  for requested_id in "${REQUESTED_SCENARIOS[@]}"; do
    found=false
    for scenario_id in "${MANIFEST_IDS[@]}"; do
      if [[ "${scenario_id}" == "${requested_id}" ]]; then
        found=true
        break
      fi
    done
    if [[ "${found}" != "true" ]]; then
      echo "ERROR: unsupported K8s dynamic scenario: ${requested_id}" >&2
      exit 1
    fi
    for selected_id in ${SELECTED_SCENARIOS[@]+"${SELECTED_SCENARIOS[@]}"}; do
      if [[ "${selected_id}" == "${requested_id}" ]]; then
        echo "ERROR: dynamic scenario requested more than once: ${requested_id}" >&2
        exit 1
      fi
    done
    SELECTED_SCENARIOS+=("${requested_id}")
  done
fi

if [[ ! -f "${PORTS_CONFIG}" ]]; then
  echo "ERROR: ports.json not found: ${PORTS_CONFIG}" >&2
  exit 1
fi
if ! command -v kubectl >/dev/null 2>&1; then
  echo "ERROR: kubectl not found in PATH" >&2
  exit 1
fi
if ! command -v tar >/dev/null 2>&1; then
  echo "ERROR: tar not found in PATH" >&2
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq not found in PATH" >&2
  exit 1
fi
case "${SYNC_TIMEOUT_SECONDS}:${SYNC_POLL_INTERVAL_SECONDS}:${PHASE_EXEC_TIMEOUT_SECONDS}" in
  *[!0-9:]*|0:*|*:0:*|*:0)
    echo "ERROR: sync, poll, and phase execution timeouts must be positive integer seconds" >&2
    exit 1
    ;;
esac

if [[ -z "${POD}" ]]; then
  POD="$(
    kubectl get pods -n "${NAMESPACE}" -l app=edgion-test-client \
      --field-selector=status.phase=Running \
      --sort-by=.metadata.creationTimestamp \
      -o custom-columns=NAME:.metadata.name,DELETING:.metadata.deletionTimestamp \
      --no-headers 2>/dev/null \
      | awk '$2=="<none>" {print $1}' \
      | tail -n 1
  )"
fi

IFS=$'\t' read -r PEER_UID PEER_IP PEER_HIT_CIDR PEER_MISS_CIDR < <(CONF_ROOT="${CONF_ROOT}" TEST_CLIENT_NAMESPACE="${NAMESPACE}" "${SCRIPT_DIR}/test_client_peer_identity.sh" environment)
[[ -n "${PEER_UID}" && -n "${PEER_IP}" && -n "${PEER_HIT_CIDR}" && -n "${PEER_MISS_CIDR}" ]] || { echo "Missing prepared peer identity" >&2; exit 1; }
SELECTED_PEER="$(kubectl get pod "${POD}" --namespace "${NAMESPACE}" --output json --request-timeout=5s | jq -r '[.metadata.uid, .status.podIP] | @tsv')"
[[ "${SELECTED_PEER}" == "${PEER_UID}"$'\t'"${PEER_IP}" ]] || { echo "Selected Pod does not match prepared Ready peer identity" >&2; exit 1; }

DIRECT_ENDPOINT_IP="127.0.0.1"
DIRECT_UNREADY_ENDPOINT_IP="127.0.0.1"
REGION_BACKEND_IP="127.0.0.1"
BACKEND_ENDPOINT_IPS=""
needs_endpoint_discovery=false
for scenario_id in "${SELECTED_SCENARIOS[@]}"; do
  case "${scenario_id}" in
    DirectEndpointPortResolution|RegionRouteValidation) needs_endpoint_discovery=true ;;
    ResponseCacheDynamic|DslQueryLookupDynamic|AiGuardOrderingDynamic|AiMessagesBinding|AiResponsesContinuation|DslXmlDynamic)
      BACKEND_ENDPOINT_IPS="$(
        "${ENDPOINT_INVENTORY}" --namespace edgion-test \
          --service edgion-test-server --port 30001 --timeout-secs 30 --output ips
      )"
      ;;
  esac
done
if [[ "${needs_endpoint_discovery}" == "true" ]]; then
  DIRECT_ENDPOINT_IP="$(
    kubectl get pods -n edgion-test -l app=edgion-test-server \
      --field-selector=status.phase=Running -o json \
      | jq -r '[.items[] | select(.metadata.deletionTimestamp == null and .status.podIP != null) | .status.podIP] | sort | first // empty'
  )"
  if [[ -z "${DIRECT_ENDPOINT_IP}" ]]; then
    echo "ERROR: no live edgion-test-server Pod IP is available for dynamic endpoint assertions" >&2
    exit 1
  fi
  if printf '%s\n' "${SELECTED_SCENARIOS[@]}" | grep -Fxq DirectEndpointPortResolution; then
    DIRECT_UNREADY_ENDPOINT_IP="$(
      kubectl get pods -n edgion-test -l app=direct-port-unready \
        --field-selector=status.phase=Running -o json \
        | jq -r '[.items[] | select(.metadata.deletionTimestamp == null and .status.podIP != null) | .status.podIP] | sort | first // empty'
    )"
    if [[ -z "${DIRECT_UNREADY_ENDPOINT_IP}" ]]; then
      echo "ERROR: no running direct-port-unready Pod IP is available for DirectEndpoint assertions" >&2
      exit 1
    fi
  fi
  REGION_BACKEND_IP="$(kubectl get service edgion-test-server -n edgion-test -o jsonpath='{.spec.clusterIP}')"
  if [[ -z "${REGION_BACKEND_IP}" || "${REGION_BACKEND_IP}" == "None" ]]; then
    echo "ERROR: edgion-test-server has no stable ClusterIP for RegionRoute validation" >&2
    exit 1
  fi
fi
if [[ -z "${POD}" ]]; then
  echo "ERROR: cannot find a running test-client pod in namespace ${NAMESPACE}" >&2
  exit 1
fi

GATEWAY_CAPABILITIES="${EDGION_TEST_GATEWAY_CAPABILITIES:-}"
if [[ -z "${GATEWAY_CAPABILITIES}" ]]; then
  GATEWAY_CAPABILITIES="$(
    kubectl exec -n edgion-system deployment/edgion-gateway -c edgion-gateway -- sh -c '
      if [ -x /usr/local/edgion/edgion-gateway ]; then
        exec /usr/local/edgion/edgion-gateway --print-build-capabilities
      fi
      exec /usr/local/bin/edgion-gateway --print-build-capabilities
    '
  )"
fi
if [[ -z "${GATEWAY_CAPABILITIES}" ]]; then
  echo "ERROR: Gateway capability probe returned an empty result" >&2
  exit 1
fi

registry_output="$(
  kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
    "${TEST_CLIENT_BIN_PATH}" --list-dynamic-scenarios
)" || {
  echo "ERROR: failed to enumerate the test-client dynamic scenario registry" >&2
  exit 1
}
if [[ -z "${registry_output}" ]]; then
  echo "ERROR: test-client dynamic scenario registry is empty" >&2
  exit 1
fi
for scenario_id in "${SELECTED_SCENARIOS[@]}"; do
  if ! printf '%s\n' "${registry_output}" | grep -Fxq -- "${scenario_id}"; then
    echo "ERROR: test-client dynamic registry does not contain ${scenario_id}" >&2
    exit 1
  fi
done
if ! kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
  sh -c 'command -v curl >/dev/null 2>&1'; then
  echo "ERROR: curl is unavailable in the test-client pod" >&2
  exit 1
fi

REMOTE_ROOT="/tmp/edgion-k8s-dynamic-$(date +%Y%m%d%H%M%S)-$$"
REMOTE_CONF_ROOT="${REMOTE_ROOT}/edgion-tests/integration/conf"
REMOTE_PREPARED=false
DIRTY_SCENARIOS=()
PHASE_EXEC_PID=""
PHASE_WATCHDOG_PID=""
REMOTE_PHASE_PID_FILE="${REMOTE_ROOT}/phase-process.pid"
LOCAL_PHASE_DIR=""
REMOTE_PHASE_CONTAINMENT_FAILED=false
REMOTE_PHASE_POD_DELETED=false

load_scenario_metadata() {
  local requested_id="$1"
  local index=0

  while [[ "${index}" -lt "${#MANIFEST_IDS[@]}" ]]; do
    if [[ "${MANIFEST_IDS[${index}]}" == "${requested_id}" ]]; then
      SCENARIO_CONF_PATH="${MANIFEST_PATHS[${index}]}"
      SCENARIO_RESET_PATH="${MANIFEST_RESET_PATHS[${index}]}"
      SCENARIO_KUBECTL_RESOURCE="${MANIFEST_KUBECTL_RESOURCES[${index}]}"
      SCENARIO_GATEWAY_KIND="${MANIFEST_GATEWAY_KINDS[${index}]}"
      SCENARIO_NAMESPACE="${MANIFEST_NAMESPACES[${index}]}"
      SCENARIO_NAME="${MANIFEST_NAMES[${index}]}"
      return 0
    fi
    index=$((index + 1))
  done

  echo "ERROR: internal manifest lookup failed for ${requested_id}" >&2
  return 1
}

mark_scenario_dirty() {
  local scenario_id="$1"
  local dirty_id
  for dirty_id in ${DIRTY_SCENARIOS[@]+"${DIRTY_SCENARIOS[@]}"}; do
    [[ "${dirty_id}" == "${scenario_id}" ]] && return 0
  done
  DIRTY_SCENARIOS+=("${scenario_id}")
}

mark_scenario_clean() {
  local scenario_id="$1"
  local dirty_id
  local remaining=()
  for dirty_id in ${DIRTY_SCENARIOS[@]+"${DIRTY_SCENARIOS[@]}"}; do
    [[ "${dirty_id}" == "${scenario_id}" ]] || remaining+=("${dirty_id}")
  done
  DIRTY_SCENARIOS=(${remaining[@]+"${remaining[@]}"})
}

wait_for_gateway_resource_version() {
  local scenario_id="$1"
  local deadline=$((SECONDS + SYNC_TIMEOUT_SECONDS))
  local live_resource_version=""
  local gateway_json=""
  local gateway_resource_version=""
  local gateway_match_count="0"

  while [[ "${SECONDS}" -lt "${deadline}" ]]; do
    if [[ "${SCENARIO_NAMESPACE}" == "-" ]]; then
      live_resource_version="$(
        kubectl get "${SCENARIO_KUBECTL_RESOURCE}" "${SCENARIO_NAME}" \
          -o jsonpath='{.metadata.resourceVersion}' 2>/dev/null || true
      )"
    else
      live_resource_version="$(
        kubectl get "${SCENARIO_KUBECTL_RESOURCE}" \
          -n "${SCENARIO_NAMESPACE}" "${SCENARIO_NAME}" \
          -o jsonpath='{.metadata.resourceVersion}' 2>/dev/null || true
      )"
    fi
    gateway_json="$(
      kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
        curl --fail --silent --show-error --max-time 5 \
        -H "Authorization: Bearer ${GATEWAY_ADMIN_TOKEN}" \
        "${GATEWAY_ADMIN_URL%/}/configclient/${SCENARIO_GATEWAY_KIND}/list" 2>/dev/null || true
    )"

    if [[ -n "${live_resource_version}" && -n "${gateway_json}" ]] \
      && printf '%s\n' "${gateway_json}" \
        | jq -e '.success == true and (.data | type == "array") and (.count == (.data | length))' \
          >/dev/null 2>&1; then
      gateway_match_count="$(
        printf '%s\n' "${gateway_json}" \
          | jq -r --arg namespace "${SCENARIO_NAMESPACE}" --arg name "${SCENARIO_NAME}" \
            '[.data[] | select(
              (($namespace == "-" and ((.metadata.namespace // "") == "")) or
               (.metadata.namespace == $namespace)) and
              .metadata.name == $name
            )] | length'
      )"
      gateway_resource_version="$(
        printf '%s\n' "${gateway_json}" \
          | jq -r --arg namespace "${SCENARIO_NAMESPACE}" --arg name "${SCENARIO_NAME}" \
            '.data[] | select(
              (($namespace == "-" and ((.metadata.namespace // "") == "")) or
               (.metadata.namespace == $namespace)) and
              .metadata.name == $name
            ) | .metadata.resourceVersion // empty'
      )"
      if [[ "${gateway_match_count}" == "1" \
        && "${gateway_resource_version}" == "${live_resource_version}" ]]; then
        echo "Gateway converged for ${scenario_id} at resourceVersion ${live_resource_version}"
        return 0
      fi
    fi

    sleep "${SYNC_POLL_INTERVAL_SECONDS}"
  done

  echo "ERROR: Gateway did not converge for ${scenario_id} within ${SYNC_TIMEOUT_SECONDS}s" >&2
  echo "ERROR: live resourceVersion=${live_resource_version:-missing}, gateway resourceVersion=${gateway_resource_version:-missing}, matches=${gateway_match_count}" >&2
  return 1
}

delete_reset_absent_resources() {
  local scenario_id="$1"
  local delete_list="${CONF_ROOT}/${SCENARIO_CONF_PATH}/${SCENARIO_RESET_PATH}/reset-delete.txt"
  local line=""
  local resource=""
  local resource_namespace=""
  local resource_name=""
  local extra=""

  # An optional reset-delete.txt beside the reset YAML lists resources that
  # must be absent in the initial phase. The file uses Resource/Namespace/Name;
  # an empty namespace segment denotes a cluster-scoped resource.
  [[ -f "${delete_list}" ]] || return 0

  while IFS= read -r line || [[ -n "${line}" ]]; do
    line="${line%$'\r'}"
    [[ -z "${line}" || "${line}" == \#* ]] && continue
    IFS=/ read -r resource resource_namespace resource_name extra <<< "${line}"
    if [[ -z "${resource}" || -z "${resource_name}" || -n "${extra}" ]]; then
      echo "ERROR: invalid reset-delete row for ${scenario_id}: ${line}" >&2
      return 1
    fi
    case "${resource}:${resource_namespace}:${resource_name}" in
      *[!A-Za-z0-9._:-]*)
        echo "ERROR: invalid reset-delete identity for ${scenario_id}: ${line}" >&2
        return 1
        ;;
    esac

    echo "Deleting reset-absent resource for ${scenario_id}: ${line}"
    if [[ -n "${resource_namespace}" ]]; then
      kubectl delete "${resource}" -n "${resource_namespace}" "${resource_name}" \
        --ignore-not-found --wait=true || return 1
    else
      kubectl delete "${resource}" "${resource_name}" --ignore-not-found --wait=true || return 1
    fi
  done < "${delete_list}"
}

restore_scenario_initial() {
  local scenario_id="$1"
  local reset_source rendered_reset rendered_root=""

  load_scenario_metadata "${scenario_id}" || return 1
  echo "Restoring initial K8s fixtures for ${scenario_id}"
  if ! delete_reset_absent_resources "${scenario_id}"; then
    echo "ERROR: failed to delete reset-absent resources for ${scenario_id}" >&2
    return 1
  fi
  reset_source="${CONF_ROOT}/${SCENARIO_CONF_PATH}/${SCENARIO_RESET_PATH}"
  rendered_reset="${reset_source}"
  if [[ "${scenario_id}" == "RegionRouteValidation" \
    || -n "${TEST_SERVER_IMAGE}" || -n "${PULL_POLICY_OVERRIDE}" ]]; then
    rendered_root="$(mktemp -d /tmp/edgion-k8s-dynamic-reset.XXXXXX)"
    rendered_reset="${rendered_root}/${SCENARIO_CONF_PATH}/${SCENARIO_RESET_PATH}"
    mkdir -p "${rendered_reset}"
    cp -R "${reset_source}/." "${rendered_reset}/"
    while IFS= read -r fixture; do
      sed -i.bak "s/__EDGION_TEST_REGION_BACKEND_IP__/${REGION_BACKEND_IP}/g" "${fixture}"
      rm -f "${fixture}.bak"
    done < <(find "${rendered_reset}" -type f \( -name '*.yaml' -o -name '*.yml' \))
  fi
  if [[ -n "${rendered_root}" && ( -n "${TEST_SERVER_IMAGE}" || -n "${PULL_POLICY_OVERRIDE}" ) ]]; then
    python3 - "${rendered_reset}" "${TEST_SERVER_IMAGE}" "${PULL_POLICY_OVERRIDE}" <<'PY'
import os
import sys

import yaml

root, image, pull_policy = sys.argv[1:]
for directory, _, filenames in os.walk(root):
    for filename in filenames:
        if not filename.endswith((".yaml", ".yml")):
            continue
        path = os.path.join(directory, filename)
        with open(path, encoding="utf-8") as stream:
            documents = list(yaml.safe_load_all(stream))
        changed = False
        for document in documents:
            if not isinstance(document, dict) or document.get("kind") != "Deployment":
                continue
            labels = document.get("metadata", {}).get("labels", {})
            if labels.get("edgion.io/test-server-image") != "true":
                continue
            containers = document.get("spec", {}).get("template", {}).get("spec", {}).get("containers", [])
            for container in containers:
                if container.get("name") != "test-server":
                    continue
                if image:
                    container["image"] = image
                if pull_policy:
                    container["imagePullPolicy"] = pull_policy
                changed = True
        if changed:
            with open(path, "w", encoding="utf-8") as stream:
                yaml.safe_dump_all(documents, stream, sort_keys=False)
PY
  fi
  if ! kubectl apply --server-side --force-conflicts \
    --field-manager=edgion-k8s-dynamic-reset --recursive \
    -f "${rendered_reset}"; then
    [[ -z "${rendered_root}" ]] || rm -rf "${rendered_root}"
    echo "ERROR: failed to restore initial K8s fixtures for ${scenario_id}" >&2
    return 1
  fi
  [[ -z "${rendered_root}" ]] || rm -rf "${rendered_root}"
  wait_for_gateway_resource_version "${scenario_id}"
}

prepare_ext_proc_external() {
  local processor_pod=""
  local material_dir="/var/lib/edgion/ext-proc-external-tls"
  kubectl rollout status deployment/ext-proc-external -n edgion-test --timeout=60s || return 1
  processor_pod="$(
    kubectl get pods -n edgion-test -l app=ext-proc-external \
      --field-selector=status.phase=Running -o json \
      | jq -r '[.items[] | select(.metadata.deletionTimestamp == null and
          any(.status.conditions[]?; .type == "Ready" and .status == "True"))
          | .metadata.name] | sort | first // empty'
  )"
  if [[ -z "${processor_pod}" ]]; then
    echo "ERROR: no ready independent ExtProc processor Pod" >&2
    return 1
  fi
  if ! kubectl exec -n edgion-test "${processor_pod}" -c processor -- \
    cat "${material_dir}/ext-proc-external-server-ca.yaml" \
    | kubectl apply --server-side --force-conflicts -f - >/dev/null; then
    echo "ERROR: failed to install external ExtProc server CA Secret" >&2
    return 1
  fi
}

run_phase_endpoint_inventory() {
  local scenario_id="$1"
  local phase="$2"
  local inventory_file="${CONF_ROOT}/${SCENARIO_CONF_PATH}/endpoint-inventory.tsv"
  local row_phase=""
  local endpoint_namespace=""
  local service=""
  local port=""
  local expected_count=""
  local extra=""
  local matched=0
  local output=""
  local inventory_args=()

  [[ -f "${inventory_file}" ]] || return 0
  if [[ ! -x "${ENDPOINT_INVENTORY}" ]]; then
    echo "ERROR: EndpointSlice inventory helper is not executable: ${ENDPOINT_INVENTORY}" >&2
    return 1
  fi

  while IFS=$'\t' read -r row_phase endpoint_namespace service port expected_count extra \
    || [[ -n "${row_phase}" ]]; do
    row_phase="${row_phase%$'\r'}"
    endpoint_namespace="${endpoint_namespace%$'\r'}"
    service="${service%$'\r'}"
    port="${port%$'\r'}"
    expected_count="${expected_count%$'\r'}"
    extra="${extra%$'\r'}"
    [[ -z "${row_phase}" || "${row_phase}" == \#* || "${row_phase}" != "${phase}" ]] && continue
    if [[ -z "${endpoint_namespace}" || -z "${service}" || -z "${port}" \
      || -z "${expected_count}" || -n "${extra}" ]]; then
      echo "ERROR: invalid endpoint inventory row for ${scenario_id}/${phase}" >&2
      return 1
    fi
    case "${port}" in
      ''|*[!0-9]*)
        echo "ERROR: endpoint inventory port must be a numeric port for ${scenario_id}/${phase}" >&2
        return 1
        ;;
    esac
    if (( 10#${port} < 1 || 10#${port} > 65535 )); then
      echo "ERROR: endpoint inventory port is outside 1-65535 for ${scenario_id}/${phase}" >&2
      return 1
    fi
    case "${row_phase}:${endpoint_namespace}:${service}:${port}" in
      *[!A-Za-z0-9._:-]*)
        echo "ERROR: invalid endpoint inventory identity for ${scenario_id}/${phase}" >&2
        return 1
        ;;
    esac
    case "${expected_count}" in
      nonempty) ;;
      ''|*[!0-9]*)
        echo "ERROR: endpoint inventory expected count must be nonempty or an integer for ${scenario_id}/${phase}" >&2
        return 1
        ;;
    esac

    inventory_args=(
      --namespace "${endpoint_namespace}"
      --service "${service}"
      --port "${port}"
      --timeout-secs "${SYNC_TIMEOUT_SECONDS}"
      --interval-secs "${SYNC_POLL_INTERVAL_SECONDS}"
      --output hostports
    )
    if [[ "${expected_count}" != "nonempty" ]]; then
      inventory_args+=(--expected-count "${expected_count}")
    fi
    echo "Reading live EndpointSlice inventory for ${scenario_id}/${phase}: ${endpoint_namespace}/${service}:${port}"
    if ! output="$("${ENDPOINT_INVENTORY}" "${inventory_args[@]}")"; then
      return 1
    fi
    echo "Live EndpointSlice inventory for ${scenario_id}/${phase}: ${output:-<empty>}"
    matched=$((matched + 1))
  done < "${inventory_file}"

  if [[ "${matched}" -eq 0 ]]; then
    echo "ERROR: endpoint inventory has no row for ${scenario_id}/${phase}" >&2
    return 1
  fi
}

run_phase_action() {
  local scenario_id="$1"
  local phase="$2"
  if [[ "${scenario_id}/${phase}" == "CorsValidation/admin_rejected" ]]; then
    kubectl patch edgionplugins.edgion.io cors-validation-dynamic -n edgion-default \
      --request-timeout=10s --type=json \
      -p '[{"op":"test","path":"/spec/requestPlugins/0/type","value":"Cors"},{"op":"replace","path":"/spec/requestPlugins/0/config/allowCredentials","value":{"invalid":true}}]'
  else
    run_phase_endpoint_inventory "${scenario_id}" "${phase}"
  fi
}

run_scenario_with_phase_barriers() {
  local scenario_id="$1"
  local line=""
  local marker=""
  local marker_scenario=""
  local marker_phase=""
  local marker_extra=""
  local phase_failed=false
  local exec_rc=0
  local watchdog_rc=0
  local input_fifo=""
  local output_fifo=""
  local timeout_marker=""
  local containment_marker=""
  local pod_deleted_marker=""

  LOCAL_PHASE_DIR="$(mktemp -d /tmp/edgion-k8s-dynamic-phase.XXXXXX)"
  input_fifo="${LOCAL_PHASE_DIR}/input"
  output_fifo="${LOCAL_PHASE_DIR}/output"
  timeout_marker="${LOCAL_PHASE_DIR}/timed-out"
  containment_marker="${LOCAL_PHASE_DIR}/containment-failed"
  pod_deleted_marker="${LOCAL_PHASE_DIR}/pod-deleted"
  mkfifo "${input_fifo}" "${output_fifo}"
  exec 9<>"${input_fifo}"

  kubectl exec -i -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
    env EDGION_TEST_ADMIN_API_URL="${CONTROLLER_ADMIN_URL}" \
    EDGION_TEST_ADMIN_TOKEN="${CONTROLLER_ADMIN_TOKEN}" \
    EDGION_TOKEN="${CONTROLLER_ADMIN_TOKEN}" \
    EDGION_GATEWAY_ADMIN_TOKEN="${GATEWAY_ADMIN_TOKEN}" \
    EDGION_TEST_GATEWAY_ADMIN_URL="${GATEWAY_ADMIN_URL}" \
    EDGION_TEST_GATEWAY_CAPABILITIES="${GATEWAY_CAPABILITIES}" \
    EDGION_TEST_K8S_PEER_UID="${PEER_UID}" \
    EDGION_TEST_K8S_PEER_IP="${PEER_IP}" \
    EDGION_TEST_K8S_PEER_HIT_CIDR="${PEER_HIT_CIDR}" \
    EDGION_TEST_K8S_PEER_MISS_CIDR="${PEER_MISS_CIDR}" \
    EDGION_TEST_K8S_MODE=true \
    EDGION_TEST_GATEWAY_RESTART_GUARD=true \
    EDGION_TEST_DNS_FLAP_SERVER_HOST="${EDGION_TEST_DNS_FLAP_SERVER_HOST:-test-http-retry.edgion-test.svc.cluster.local}" \
    EDGION_EXT_PROC_EXTERNAL_REQUIRED=1 \
    EDGION_EXT_PROC_EXTERNAL_ADMIN_URL="http://ext-proc-external.edgion-test.svc.cluster.local:30045" \
    EDGION_TEST_DIRECT_ENDPOINT_IP="${DIRECT_ENDPOINT_IP}" \
    EDGION_TEST_DIRECT_UNREADY_ENDPOINT_IP="${DIRECT_UNREADY_ENDPOINT_IP}" \
    EDGION_TEST_REGION_BACKEND_IP="${REGION_BACKEND_IP}" \
    EDGION_TEST_BACKEND_ENDPOINT_IPS="${BACKEND_ENDPOINT_IPS}" \
    EDGION_TEST_MIRROR_SERVER_HOST="edgion-test-server.edgion-test.svc.cluster.local" \
    EDGION_TEST_K8S_PHASE_BARRIER=true \
    EDGION_TEST_K8S_PHASE_ACK_TIMEOUT_SECONDS="${PHASE_EXEC_TIMEOUT_SECONDS}" \
    sh -c '
      cd "$1" || exit 1
      exec 8<&0
      "$2" -g --target-host "$3" --dynamic-scenario "$4" <&8 &
      child_pid=$!
      child_start=""
      while [ -z "$child_start" ] && kill -0 "$child_pid" 2>/dev/null; do
        child_start=$(awk "{print \$22}" "/proc/$child_pid/stat" 2>/dev/null || true)
      done
      if [ -z "$child_start" ]; then
        wait "$child_pid"
        exit $?
      fi
      printf "%s %s\n" "$child_pid" "$child_start" > "$5"
      wait "$child_pid"
      child_rc=$?
      rm -f "$5"
      exit "$child_rc"
    ' sh "${REMOTE_ROOT}" "${TEST_CLIENT_BIN_PATH}" "${TARGET_HOST}" "${scenario_id}" \
    "${REMOTE_PHASE_PID_FILE}" \
    < "${input_fifo}" > "${output_fifo}" 2>&1 &
  PHASE_EXEC_PID="$!"
  (
    local deadline=$((SECONDS + PHASE_EXEC_TIMEOUT_SECONDS))
    while kill -0 "${PHASE_EXEC_PID}" >/dev/null 2>&1; do
      if [[ "${SECONDS}" -ge "${deadline}" ]]; then
        : > "${timeout_marker}"
        echo "ERROR: dynamic scenario ${scenario_id} exceeded ${PHASE_EXEC_TIMEOUT_SECONDS}s execution deadline" >&2
        if contain_remote_phase_process; then
          :
        else
          containment_rc=$?
          if [[ "${containment_rc}" -eq 2 ]]; then
            : > "${pod_deleted_marker}"
          else
            : > "${containment_marker}"
            echo "ERROR: remote dynamic process containment failed for ${NAMESPACE}/${POD}" >&2
          fi
        fi
        kill "${PHASE_EXEC_PID}" >/dev/null 2>&1 || true
        sleep 2
        kill -9 "${PHASE_EXEC_PID}" >/dev/null 2>&1 || true
        exit 124
      fi
      sleep 1
    done
  ) &
  PHASE_WATCHDOG_PID="$!"

  while IFS= read -r line || [[ -n "${line}" ]]; do
    printf '%s\n' "${line}"
    case "${line}" in
      '@@DYNAMIC_PHASE '*)
        read -r marker marker_scenario marker_phase marker_extra <<< "${line}"
        if [[ "${marker}" != "@@DYNAMIC_PHASE" || "${marker_scenario}" != "${scenario_id}" \
          || -z "${marker_phase}" || -n "${marker_extra}" ]]; then
          echo "ERROR: malformed dynamic phase marker: ${line}" >&2
          phase_failed=true
          printf '@@DYNAMIC_PHASE_ACK %s %s ERROR\n' "${scenario_id}" "${marker_phase:-unknown}" >&9
          continue
        fi
        if run_phase_action "${scenario_id}" "${marker_phase}"; then
          printf '@@DYNAMIC_PHASE_ACK %s %s OK\n' "${scenario_id}" "${marker_phase}" >&9
        else
          phase_failed=true
          printf '@@DYNAMIC_PHASE_ACK %s %s ERROR\n' "${scenario_id}" "${marker_phase}" >&9
        fi
        ;;
    esac
  done < "${output_fifo}"

  # Stop and reap the watchdog before reaping the exec PID. This keeps the exec
  # PID owned by this shell throughout the watchdog's TERM/KILL escalation, so
  # the numeric PID cannot be reused for an unrelated host process.
  if [[ ! -f "${timeout_marker}" ]]; then
    kill "${PHASE_WATCHDOG_PID}" >/dev/null 2>&1 || true
  fi
  wait "${PHASE_WATCHDOG_PID}" || watchdog_rc=$?
  PHASE_WATCHDOG_PID=""
  wait "${PHASE_EXEC_PID}" || exec_rc=$?
  PHASE_EXEC_PID=""
  if [[ "${watchdog_rc}" -eq 124 ]]; then
    exec_rc=124
  fi
  if [[ -f "${containment_marker}" ]]; then
    REMOTE_PHASE_CONTAINMENT_FAILED=true
    exec_rc=125
  fi
  if [[ -f "${pod_deleted_marker}" ]]; then
    REMOTE_PHASE_POD_DELETED=true
  fi
  exec 9>&-
  rm -rf -- "${LOCAL_PHASE_DIR}"
  LOCAL_PHASE_DIR=""
  if [[ "${phase_failed}" == "true" && "${exec_rc}" -eq 0 ]]; then
    exec_rc=1
  fi
  return "${exec_rc}"
}

terminate_remote_phase_process() {
  kubectl --request-timeout=10s exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
    sh -c '
      state_file=$1
      [ -f "$state_file" ] || exit 1
      read -r pid expected_start < "$state_file" || exit 1
      inspect_process() {
        [ -e "/proc/$pid" ] || return 1
        [ -r "/proc/$pid/stat" ] || return 2
        current_start=$(awk "{print \$22}" "/proc/$pid/stat" 2>/dev/null) || return 2
        case "$current_start" in
          ""|*[!0-9]*) return 2 ;;
        esac
        [ "$current_start" = "$expected_start" ] && return 0
        return 1
      }
      if inspect_process; then
        kill "$pid" 2>/dev/null || true
        attempts=0
        while inspect_process && [ "$attempts" -lt 5 ]; do
          sleep 1
          attempts=$((attempts + 1))
        done
        inspect_rc=0
        inspect_process || inspect_rc=$?
        [ "$inspect_rc" -eq 2 ] && exit 1
        if [ "$inspect_rc" -eq 0 ]; then
          kill -9 "$pid" 2>/dev/null || true
          sleep 1
        fi
      fi
      inspect_rc=0
      inspect_process || inspect_rc=$?
      [ "$inspect_rc" -eq 1 ] || exit 1
      rm -f "$state_file"
    ' sh "${REMOTE_PHASE_PID_FILE}"
}

contain_remote_phase_process() {
  if terminate_remote_phase_process; then
    return 0
  fi
  echo "ERROR: remote dynamic process could not be confirmed stopped; deleting ${NAMESPACE}/${POD}" >&2
  if ! kubectl delete pod -n "${NAMESPACE}" "${POD}" --wait=true --timeout=60s; then
    echo "ERROR: failed to delete ${NAMESPACE}/${POD}; fixture restoration is unsafe" >&2
    return 1
  fi
  return 2
}

cleanup() {
  local exit_rc=$?
  local cleanup_failed=false
  local dirty_id

  trap - EXIT INT TERM
  set +e
  if [[ -n "${PHASE_WATCHDOG_PID}" ]]; then
    kill "${PHASE_WATCHDOG_PID}" >/dev/null 2>&1 || true
    wait "${PHASE_WATCHDOG_PID}" >/dev/null 2>&1 || true
    PHASE_WATCHDOG_PID=""
  fi
  if [[ -n "${PHASE_EXEC_PID}" ]]; then
    if contain_remote_phase_process >/dev/null 2>&1; then
      :
    else
      containment_rc=$?
      if [[ "${containment_rc}" -eq 2 ]]; then
        REMOTE_PHASE_POD_DELETED=true
      else
        REMOTE_PHASE_CONTAINMENT_FAILED=true
        cleanup_failed=true
      fi
    fi
    kill "${PHASE_EXEC_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${PHASE_EXEC_PID}" ]]; then
    wait "${PHASE_EXEC_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${LOCAL_PHASE_DIR}" ]]; then
    rm -rf -- "${LOCAL_PHASE_DIR}"
  fi
  if [[ "${REMOTE_PHASE_CONTAINMENT_FAILED}" == "true" ]]; then
    echo "ERROR: skipping fixture restoration because remote process containment was not established" >&2
  else
    for dirty_id in ${DIRTY_SCENARIOS[@]+"${DIRTY_SCENARIOS[@]}"}; do
      if restore_scenario_initial "${dirty_id}"; then
        mark_scenario_clean "${dirty_id}"
      else
        cleanup_failed=true
      fi
    done
  fi
  if [[ "${REMOTE_PREPARED}" == "true" ]]; then
    if [[ "${REMOTE_PHASE_POD_DELETED}" != "true" ]]; then
      kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
        rm -rf "${REMOTE_ROOT}" >/dev/null 2>&1 || true
    fi
  fi
  if [[ "${cleanup_failed}" == "true" && "${exit_rc}" -eq 0 ]]; then
    exit_rc=1
  fi
  exit "${exit_rc}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
  mkdir -p "${REMOTE_CONF_ROOT}"
REMOTE_PREPARED=true

# Only the validated K8s conf tree is archived. COPYFILE_DISABLE and the tar
# flags prevent macOS xattrs from becoming `._*` AppleDouble YAML files in the
# pod. The canonical ports registry is copied separately because it is JSON
# metadata, not a Kubernetes resource.
COPYFILE_DISABLE=1 tar --no-xattrs --exclude='._*' --exclude='__MACOSX' \
  -C "${CONF_ROOT}" -cf - . | kubectl exec -i -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
  tar -xf - -C "${REMOTE_CONF_ROOT}"

kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
  sh -c 'find "$1" -type f \( -name "*.yaml" -o -name "*.yml" \) -exec sed -i "s/__EDGION_TEST_REGION_BACKEND_IP__/$2/g" {} +' \
  sh "${REMOTE_CONF_ROOT}" "${REGION_BACKEND_IP}"

metadata_entries="$(
  kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
    find "${REMOTE_CONF_ROOT}" \( -name '._*' -o -name '__MACOSX' \) -print
)"
if [[ -n "${metadata_entries}" ]]; then
  echo "ERROR: macOS archive metadata was staged as scenario input:" >&2
  printf '%s\n' "${metadata_entries}" >&2
  exit 1
fi

kubectl exec -i -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
  sh -c 'cat > "$1"' sh "${REMOTE_CONF_ROOT}/ports.json" < "${PORTS_CONFIG}"
kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
  ln -s /usr/local/edgion/edgion-tests/certs "${REMOTE_ROOT}/edgion-tests/certs"

total=0
failed=0
gateway_process_identity() {
  kubectl get pods -n edgion-system -l app=edgion-gateway --request-timeout=5s -o json | jq -ce '
    [.items[] | select(.metadata.deletionTimestamp == null) |
      {uid: .metadata.uid, containers: [.status.containerStatuses[]? |
        {name, restartCount, containerID, running: .state.running.startedAt}]}] | sort_by(.uid) |
    if length > 0 and all(.[]; (.containers | length) > 0 and all(.containers[]; .running != null))
    then . else error("Gateway process identity unavailable") end'
}
for scenario_id in "${SELECTED_SCENARIOS[@]}"; do
  total=$((total + 1))
  scenario_rc=0
  gateway_identity_before=""
  if [[ "${scenario_id}" == "SharedHttpPolicyAdmission" ]]; then
    gateway_identity_before="$(gateway_process_identity)" || scenario_rc=1
  fi
  echo "@@DYNAMIC_START ${scenario_id}"
  mark_scenario_dirty "${scenario_id}"
  if [[ "${scenario_id}" == "ExtProcTlsPolicyRecovery" ]]; then
    prepare_ext_proc_external || scenario_rc=1
  fi
  if [[ "${scenario_rc}" -eq 0 ]] && restore_scenario_initial "${scenario_id}"; then
    mark_scenario_clean "${scenario_id}"
    mark_scenario_dirty "${scenario_id}"
    if [[ "${scenario_id}" == "CorsValidation" || -f "${CONF_ROOT}/${SCENARIO_CONF_PATH}/endpoint-inventory.tsv" ]]; then
      if run_scenario_with_phase_barriers "${scenario_id}"; then
        :
      else
        scenario_rc=$?
        [[ "${scenario_rc}" -ne 0 ]] || scenario_rc=1
      fi
    elif kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
      env EDGION_TEST_ADMIN_API_URL="${CONTROLLER_ADMIN_URL}" \
      EDGION_TEST_ADMIN_TOKEN="${CONTROLLER_ADMIN_TOKEN}" \
      EDGION_TOKEN="${CONTROLLER_ADMIN_TOKEN}" \
      EDGION_GATEWAY_ADMIN_TOKEN="${GATEWAY_ADMIN_TOKEN}" \
      EDGION_TEST_GATEWAY_ADMIN_URL="${GATEWAY_ADMIN_URL}" \
      EDGION_TEST_GATEWAY_CAPABILITIES="${GATEWAY_CAPABILITIES}" \
      EDGION_TEST_K8S_PEER_UID="${PEER_UID}" \
      EDGION_TEST_K8S_PEER_IP="${PEER_IP}" \
      EDGION_TEST_K8S_PEER_HIT_CIDR="${PEER_HIT_CIDR}" \
      EDGION_TEST_K8S_PEER_MISS_CIDR="${PEER_MISS_CIDR}" \
      EDGION_TEST_K8S_MODE=true \
      EDGION_TEST_GATEWAY_RESTART_GUARD=true \
      EDGION_TEST_DNS_FLAP_SERVER_HOST="${EDGION_TEST_DNS_FLAP_SERVER_HOST:-test-http-retry.edgion-test.svc.cluster.local}" \
      EDGION_EXT_PROC_EXTERNAL_REQUIRED=1 \
      EDGION_EXT_PROC_EXTERNAL_ADMIN_URL="http://ext-proc-external.edgion-test.svc.cluster.local:30045" \
      EDGION_TEST_DIRECT_ENDPOINT_IP="${DIRECT_ENDPOINT_IP}" \
      EDGION_TEST_DIRECT_UNREADY_ENDPOINT_IP="${DIRECT_UNREADY_ENDPOINT_IP}" \
      EDGION_TEST_REGION_BACKEND_IP="${REGION_BACKEND_IP}" \
      EDGION_TEST_BACKEND_ENDPOINT_IPS="${BACKEND_ENDPOINT_IPS}" \
      EDGION_TEST_MIRROR_SERVER_HOST="edgion-test-server.edgion-test.svc.cluster.local" \
      sh -c 'cd "$1" && exec "$2" -g --target-host "$3" --dynamic-scenario "$4"' \
      sh "${REMOTE_ROOT}" "${TEST_CLIENT_BIN_PATH}" "${TARGET_HOST}" "${scenario_id}"; then
      :
    else
      scenario_rc=$?
      [[ "${scenario_rc}" -ne 0 ]] || scenario_rc=1
    fi
    if [[ "${REMOTE_PHASE_CONTAINMENT_FAILED}" == "true" ]]; then
      scenario_rc=125
    elif restore_scenario_initial "${scenario_id}"; then
      mark_scenario_clean "${scenario_id}"
    elif [[ "${scenario_rc}" -eq 0 ]]; then
      scenario_rc=1
    fi
  else
    scenario_rc=1
  fi

  if [[ "${scenario_id}" == "SharedHttpPolicyAdmission" ]]; then
    gateway_identity_after="$(gateway_process_identity)" || scenario_rc=1
    if [[ -z "${gateway_identity_before}" || "${gateway_identity_before}" != "${gateway_identity_after}" ]]; then
      echo "ERROR: Gateway process identity changed during ${scenario_id}" >&2
      scenario_rc=1
    fi
  fi
  if [[ "${scenario_rc}" -eq 0 ]]; then
    echo "@@DYNAMIC_RESULT ${scenario_id} PASS 0"
  else
    failed=$((failed + 1))
    echo "@@DYNAMIC_RESULT ${scenario_id} FAIL ${scenario_rc}"
  fi
  if [[ "${REMOTE_PHASE_CONTAINMENT_FAILED}" == "true" || "${REMOTE_PHASE_POD_DELETED}" == "true" ]]; then
    echo "ERROR: aborting remaining dynamic scenarios after remote phase containment" >&2
    break
  fi
done

echo "@@DYNAMIC_SUMMARY total=${total} failed=${failed}"
if [[ "${failed}" -gt 0 ]]; then
  exit 1
fi
