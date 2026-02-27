#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/examples/k8stest/conf}"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/examples/k8stest/kubernetes}"
COMMON_SCRIPT_DIR="${SCRIPT_DIR}/common"
ORBSTACK_SCRIPT_DIR="${SCRIPT_DIR}/orbstack"

VALIDATE_SCRIPT="$COMMON_SCRIPT_DIR/validate_no_endpoints.sh"
DEPLOY_SCRIPT="$COMMON_SCRIPT_DIR/deploy_integration.sh"
APPLY_ALL_SCRIPT="$COMMON_SCRIPT_DIR/apply_all_conf_strict.sh"
RUN_CLIENT_SCRIPT="$COMMON_SCRIPT_DIR/run_test_client.sh"
GENERATE_CERTS_SCRIPT="$COMMON_SCRIPT_DIR/generate_runtime_certs.sh"
LOCAL_IMAGE_PREPARE_SCRIPT="$ORBSTACK_SCRIPT_DIR/prepare_local_images.sh"
GENERATED_DIR="${GENERATED_DIR:-$PROJECT_ROOT/examples/k8stest/generated}"
GENERATED_SECRET_DIR="${GENERATED_DIR}/secrets"
IMAGE_CONFIG_FILE="${IMAGE_CONFIG_FILE:-$SCRIPT_DIR/config/images.env}"
STATE_NAMESPACE="${STATE_NAMESPACE:-edgion-system}"
STATE_CONFIGMAP_NAME="${STATE_CONFIGMAP_NAME:-edgion-k8s-integration-state}"

CONTROLLER_ADMIN_URL="${EDGION_CONTROLLER_ADMIN_URL:-http://edgion-controller.edgion-system.svc.cluster.local:5800}"
GATEWAY_ADMIN_URL="${EDGION_GATEWAY_ADMIN_URL:-http://edgion-gateway.edgion-system.svc.cluster.local:5900}"

START_FROM="${START_FROM:-}"
ONLY_RESOURCE=""
ONLY_ITEM=""
SKIP_DEPLOY=false
SKIP_PREPARE=false
PREPARE_ONLY=false
TEST_SERVER_REPLICAS="${TEST_SERVER_REPLICAS:-3}"
SPEC_PROFILE="${SPEC_PROFILE:-}"
FULL_TEST=false
WITH_RELOAD=false
BACKEND_TEST_NAMESPACE="${BACKEND_TEST_NAMESPACE:-edgion-backend}"
CLUSTER_MODE="${CLUSTER_MODE:-auto}" # auto|local|remote
TEST_CLIENT_IMAGE="${TEST_CLIENT_IMAGE:-}"
TEST_SERVER_IMAGE="${TEST_SERVER_IMAGE:-}"
CONTROLLER_IMAGE="${CONTROLLER_IMAGE:-}"
GATEWAY_IMAGE="${GATEWAY_IMAGE:-}"
IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
IMAGE_VERSION="${IMAGE_VERSION:-dev}"
IMAGE_ARCH="${IMAGE_ARCH:-}"
PREPARE_LOCAL_IMAGES=false
LOCAL_IMAGE_REBUILD=false

FILTERED_MODE=false
SELECTED_COUNT=0
EXECUTED_COUNT=0
MISSING_COUNT=0
MISSING_SUITES=()

SLOW_TESTS=(
  "HTTPRoute_Backend_Timeout"
  "EdgionPlugins_AllEndpointStatus"
  "EdgionPlugins_LdapAuth"
)

show_help() {
  cat <<EOF
Usage: $0 [options]

Default flow (two-phase):
  Phase 1 Prepare: validate conf -> deploy (optional) -> strict apply all conf -> restart gateway
  Phase 2 Test: run test_client suites
Default test rounds:
  Single round by default; use --with-reload to run two rounds.

Options:
  --skip-deploy                  Skip deploy step in prepare phase
  --skip-prepare                 Skip whole prepare phase, run tests only
  --prepare-only                 Run prepare phase only, do not run tests
  --no-prepare                   Compatibility alias of --skip-prepare
  --no-start                     Compatibility alias of --skip-prepare
  --start-from <suite|res/item> Start from given suite (e.g. HTTPRoute/Match)
  -r, --resource <name>          Run only one resource (e.g. EdgionPlugins)
  -i, --item <name>              Run specific item with -r (e.g. JwtAuth)
  --test-server-replicas <n>     Deploy/scale test-server replicas (default: ${TEST_SERVER_REPLICAS})
  --spec-profile <name>          Compatibility no-op (profiles no longer used)
  --cluster-mode <auto|local|remote>
                                 Cluster mode for image compatibility checks
                                 (default: ${CLUSTER_MODE})
  --image-config <path>          Image config file (default: ${IMAGE_CONFIG_FILE})
  --prepare-local-images         Build local test images and auto-set TEST_*_IMAGE
  --rebuild-local-images         With --prepare-local-images, force rebuild
  --full-test                    Include slow tests (Timeout/AllEndpointStatus/LdapAuth)
  --with-reload                  Run test rounds twice with reload in between
  -h, --help                     Show this help

Env:
  BACKEND_TEST_NAMESPACE         Backend test namespace for deploy/cleanup scripts
                                 (default: ${BACKEND_TEST_NAMESPACE})
  TEST_CLIENT_IMAGE              Optional test-client image override (passed to deploy script)
  TEST_SERVER_IMAGE              Optional test-server image override (passed to deploy script)
  CONTROLLER_IMAGE               Optional controller image override (passed to deploy script)
  GATEWAY_IMAGE                  Optional gateway image override (passed to deploy script)
  IMAGE_CONFIG_FILE              Path to image config file
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-deploy)
      SKIP_DEPLOY=true
      shift
      ;;
    --skip-prepare)
      SKIP_PREPARE=true
      shift
      ;;
    --no-prepare)
      SKIP_PREPARE=true
      shift
      ;;
    --no-start)
      SKIP_PREPARE=true
      shift
      ;;
    --prepare-only)
      PREPARE_ONLY=true
      shift
      ;;
    --start-from)
      START_FROM="${2:-}"
      if [[ -z "${START_FROM}" ]]; then
        echo "missing value for --start-from"
        exit 1
      fi
      shift 2
      ;;
    -r|--resource)
      ONLY_RESOURCE="${2:-}"
      if [[ -z "${ONLY_RESOURCE}" ]]; then
        echo "missing value for --resource"
        exit 1
      fi
      shift 2
      ;;
    -i|--item)
      ONLY_ITEM="${2:-}"
      if [[ -z "${ONLY_ITEM}" ]]; then
        echo "missing value for --item"
        exit 1
      fi
      shift 2
      ;;
    --test-server-replicas)
      TEST_SERVER_REPLICAS="${2:-}"
      if ! [[ "${TEST_SERVER_REPLICAS}" =~ ^[0-9]+$ ]]; then
        echo "invalid --test-server-replicas: ${TEST_SERVER_REPLICAS}"
        exit 1
      fi
      shift 2
      ;;
    --spec-profile)
      SPEC_PROFILE="${2:-}"
      if [[ -z "${SPEC_PROFILE}" ]]; then
        echo "missing value for --spec-profile"
        exit 1
      fi
      shift 2
      ;;
    --cluster-mode)
      CLUSTER_MODE="${2:-}"
      if [[ -z "${CLUSTER_MODE}" ]]; then
        echo "missing value for --cluster-mode"
        exit 1
      fi
      if [[ "${CLUSTER_MODE}" != "auto" && "${CLUSTER_MODE}" != "local" && "${CLUSTER_MODE}" != "remote" ]]; then
        echo "invalid --cluster-mode: ${CLUSTER_MODE} (expected: auto|local|remote)"
        exit 1
      fi
      shift 2
      ;;
    --image-config)
      IMAGE_CONFIG_FILE="${2:-}"
      if [[ -z "${IMAGE_CONFIG_FILE}" ]]; then
        echo "missing value for --image-config"
        exit 1
      fi
      shift 2
      ;;
    --prepare-local-images)
      PREPARE_LOCAL_IMAGES=true
      shift
      ;;
    --rebuild-local-images)
      LOCAL_IMAGE_REBUILD=true
      shift
      ;;
    --full-test)
      FULL_TEST=true
      shift
      ;;
    --with-reload)
      WITH_RELOAD=true
      shift
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

if [[ -n "${ONLY_ITEM}" && -z "${ONLY_RESOURCE}" ]]; then
  echo "--item must be used with --resource"
  exit 1
fi

if [[ -n "${ONLY_RESOURCE}" ]]; then
  FILTERED_MODE=true
fi

if [[ "${SKIP_PREPARE}" == "true" && "${PREPARE_ONLY}" == "true" ]]; then
  echo "--skip-prepare and --prepare-only cannot be used together"
  exit 1
fi

for required in "$VALIDATE_SCRIPT" "$DEPLOY_SCRIPT" "$APPLY_ALL_SCRIPT" "$RUN_CLIENT_SCRIPT" "$GENERATE_CERTS_SCRIPT"; do
  if [[ ! -x "$required" ]]; then
    echo "required script not found or not executable: $required"
    exit 1
  fi
done

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found"
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq not found"
  exit 1
fi
if ! command -v curl >/dev/null 2>&1; then
  echo "curl not found"
  exit 1
fi

get_cluster_arches() {
  kubectl get nodes -o json | jq -r '.items[].status.nodeInfo.architecture' | sort -u
}

detect_cluster_mode_auto() {
  local names
  names="$(kubectl get nodes -o json | jq -r '.items[].metadata.name' | tr '[:upper:]' '[:lower:]')"
  if echo "${names}" | grep -Eq 'orbstack|docker-desktop|minikube|kind|rancher-desktop'; then
    echo "local"
  else
    echo "remote"
  fi
}

detect_cluster_mode() {
  if [[ "${CLUSTER_MODE}" == "auto" ]]; then
    detect_cluster_mode_auto
  else
    echo "${CLUSTER_MODE}"
  fi
}

load_image_config_file() {
  local f="$1"
  [[ -f "${f}" ]] || return 0

  # shellcheck disable=SC1090
  source "${f}"

  IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
  IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
  IMAGE_VERSION="${IMAGE_VERSION:-dev}"

  if [[ -z "${TEST_CLIENT_IMAGE}" ]]; then
    if [[ -n "${IMAGE_ARCH:-}" ]]; then
      TEST_CLIENT_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-test-client:${IMAGE_VERSION}_${IMAGE_ARCH}"
    fi
  fi
  if [[ -z "${TEST_SERVER_IMAGE}" ]]; then
    if [[ -n "${IMAGE_ARCH:-}" ]]; then
      TEST_SERVER_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-test-server:${IMAGE_VERSION}_${IMAGE_ARCH}"
    fi
  fi
  if [[ -z "${CONTROLLER_IMAGE}" ]]; then
    if [[ -n "${IMAGE_ARCH:-}" ]]; then
      CONTROLLER_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-controller:${IMAGE_VERSION}_${IMAGE_ARCH}"
    fi
  fi
  if [[ -z "${GATEWAY_IMAGE}" ]]; then
    if [[ -n "${IMAGE_ARCH:-}" ]]; then
      GATEWAY_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-gateway:${IMAGE_VERSION}_${IMAGE_ARCH}"
    fi
  fi
}

prepare_local_images_if_needed() {
  local mode="$1"
  if [[ "${PREPARE_LOCAL_IMAGES}" != "true" ]]; then
    return 0
  fi
  if [[ "${mode}" != "local" ]]; then
    echo "--prepare-local-images is only supported for local cluster mode (current: ${mode})"
    exit 1
  fi
  if [[ ! -x "${LOCAL_IMAGE_PREPARE_SCRIPT}" ]]; then
    echo "local image prepare script not found or not executable: ${LOCAL_IMAGE_PREPARE_SCRIPT}"
    exit 1
  fi

  local arch
  case "$(get_cluster_arches | head -n1)" in
    arm64|aarch64) arch="arm64" ;;
    amd64|x86_64) arch="amd64" ;;
    *) arch="amd64" ;;
  esac

  local args=(--arch "${arch}")
  if [[ "${LOCAL_IMAGE_REBUILD}" == "true" ]]; then
    args+=(--rebuild)
  fi
  "${LOCAL_IMAGE_PREPARE_SCRIPT}" "${args[@]}" --write-config "${IMAGE_CONFIG_FILE}"
  load_image_config_file "${IMAGE_CONFIG_FILE}"
  export TEST_CLIENT_IMAGE TEST_SERVER_IMAGE CONTROLLER_IMAGE GATEWAY_IMAGE
}

check_image_arch_compat() {
  local image="$1"
  local cluster_arch="$2"
  local role="$3"
  [[ -n "${image}" ]] || return 0

  # Heuristic by common tag suffixes from build-image.sh output.
  if [[ "${image}" =~ (_|:|-)arm64($|[^a-zA-Z0-9]) ]] && [[ "${cluster_arch}" == "amd64" || "${cluster_arch}" == "x86_64" ]]; then
    echo "[ERROR] ${role} image appears arm64 but cluster arch is ${cluster_arch}: ${image}"
    return 1
  fi
  if [[ "${image}" =~ (_|:|-)amd64($|[^a-zA-Z0-9]) ]] && [[ "${cluster_arch}" == "arm64" || "${cluster_arch}" == "aarch64" ]]; then
    echo "[ERROR] ${role} image appears amd64 but cluster arch is ${cluster_arch}: ${image}"
    return 1
  fi
  return 0
}

print_cluster_and_image_context() {
  local mode arches primary
  load_image_config_file "${IMAGE_CONFIG_FILE}"
  mode="$(detect_cluster_mode)"
  prepare_local_images_if_needed "${mode}"
  arches="$(get_cluster_arches | xargs)"
  primary="$(get_cluster_arches | head -n1)"

  echo "Cluster mode: ${mode} (configured: ${CLUSTER_MODE})"
  echo "Cluster node arch(es): ${arches}"
  echo "Image override CONTROLLER_IMAGE: ${CONTROLLER_IMAGE:-<unset>}"
  echo "Image override GATEWAY_IMAGE: ${GATEWAY_IMAGE:-<unset>}"
  echo "Image override TEST_CLIENT_IMAGE: ${TEST_CLIENT_IMAGE:-<unset>}"
  echo "Image override TEST_SERVER_IMAGE: ${TEST_SERVER_IMAGE:-<unset>}"

  check_image_arch_compat "${CONTROLLER_IMAGE}" "${primary}" "CONTROLLER_IMAGE"
  check_image_arch_compat "${GATEWAY_IMAGE}" "${primary}" "GATEWAY_IMAGE"
  check_image_arch_compat "${TEST_CLIENT_IMAGE}" "${primary}" "TEST_CLIENT_IMAGE"
  check_image_arch_compat "${TEST_SERVER_IMAGE}" "${primary}" "TEST_SERVER_IMAGE"

  if [[ "${mode}" == "remote" && "${SKIP_DEPLOY}" == "false" ]]; then
    if [[ -z "${CONTROLLER_IMAGE}" || -z "${GATEWAY_IMAGE}" || -z "${TEST_CLIENT_IMAGE}" || -z "${TEST_SERVER_IMAGE}" ]]; then
      echo "[WARN] remote mode detected without explicit test image overrides."
      echo "[WARN] recommend setting CONTROLLER_IMAGE/GATEWAY_IMAGE/TEST_CLIENT_IMAGE/TEST_SERVER_IMAGE to pushed tags."
    fi
  fi
}

hash_file() {
  local f="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${f}" | awk '{print $1}'
  else
    shasum -a 256 "${f}" | awk '{print $1}'
  fi
}

compute_prepare_conf_hash() {
  local tmp
  tmp="$(mktemp /tmp/edgion-k8s-prepare-hash.XXXXXX)"

  # Sort file list for deterministic hash.
  find "${CONF_ROOT}" -type f \( -name "*.yaml" -o -name "*.yml" \) | sort > "${tmp}"
  {
    echo "test_server_replicas=${TEST_SERVER_REPLICAS}"
    echo "backend_test_namespace=${BACKEND_TEST_NAMESPACE}"
    echo "conf_root=${CONF_ROOT}"
    while IFS= read -r f; do
      [[ -f "${f}" ]] || continue
      echo "${f}:$(hash_file "${f}")"
    done < "${tmp}"
  } | if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | awk '{print $1}'
  else
    shasum -a 256 | awk '{print $1}'
  fi

  rm -f "${tmp}"
}

write_prepare_state_marker() {
  local conf_hash="$1"
  local now_utc
  now_utc="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

  kubectl -n "${STATE_NAMESPACE}" create configmap "${STATE_CONFIGMAP_NAME}" \
    --from-literal=prepare_ok=true \
    --from-literal=conf_hash="${conf_hash}" \
    --from-literal=test_server_replicas="${TEST_SERVER_REPLICAS}" \
    --from-literal=backend_test_namespace="${BACKEND_TEST_NAMESPACE}" \
    --from-literal=updated_at="${now_utc}" \
    --dry-run=client -o yaml | kubectl apply -f -
}

get_state_data() {
  local key="$1"
  kubectl -n "${STATE_NAMESPACE}" get configmap "${STATE_CONFIGMAP_NAME}" -o "jsonpath={.data.${key}}" 2>/dev/null || true
}

verify_skip_prepare_preconditions() {
  local conf_hash expected actual
  conf_hash="$(compute_prepare_conf_hash)"

  if ! kubectl -n "${STATE_NAMESPACE}" get configmap "${STATE_CONFIGMAP_NAME}" >/dev/null 2>&1; then
    echo "--skip-prepare denied: state ConfigMap not found: ${STATE_NAMESPACE}/${STATE_CONFIGMAP_NAME}"
    echo "Run once without --skip-prepare to complete prepare phase."
    exit 1
  fi

  actual="$(get_state_data prepare_ok)"
  if [[ "${actual}" != "true" ]]; then
    echo "--skip-prepare denied: prepare_ok is not true in ${STATE_NAMESPACE}/${STATE_CONFIGMAP_NAME}"
    exit 1
  fi

  expected="${conf_hash}"
  actual="$(get_state_data conf_hash)"
  if [[ -z "${actual}" || "${actual}" != "${expected}" ]]; then
    echo "--skip-prepare denied: conf hash mismatch"
    echo "  expected(current): ${expected}"
    echo "  recorded(state):   ${actual:-<empty>}"
    echo "Run once without --skip-prepare to refresh prepare state."
    exit 1
  fi

  expected="${TEST_SERVER_REPLICAS}"
  actual="$(get_state_data test_server_replicas)"
  if [[ "${actual}" != "${expected}" ]]; then
    echo "--skip-prepare denied: test_server_replicas mismatch (current=${expected}, state=${actual:-<empty>})"
    exit 1
  fi

  expected="${BACKEND_TEST_NAMESPACE}"
  actual="$(get_state_data backend_test_namespace)"
  if [[ "${actual}" != "${expected}" ]]; then
    echo "--skip-prepare denied: BACKEND_TEST_NAMESPACE mismatch (current=${expected}, state=${actual:-<empty>})"
    exit 1
  fi

  # Runtime readiness checks.
  kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=120s >/dev/null
  kubectl rollout status deployment/edgion-test-client -n edgion-test --timeout=120s >/dev/null
  kubectl -n edgion-system get svc edgion-gateway >/dev/null
  kubectl -n edgion-test get svc edgion-test-server >/dev/null

  echo "Skip-prepare precheck passed: ${STATE_NAMESPACE}/${STATE_CONFIGMAP_NAME} is valid."
}

wait_gateway_stable() {
  local timeout_sec="${1:-300}"
  local start_ts
  start_ts="$(date +%s)"

  while true; do
    local running terminating
    running="$(kubectl get pods -n edgion-system -l app=edgion-gateway -o json | jq '[.items[] | select(.status.phase=="Running" and .metadata.deletionTimestamp==null)] | length')"
    terminating="$(kubectl get pods -n edgion-system -l app=edgion-gateway -o json | jq '[.items[] | select(.metadata.deletionTimestamp!=null)] | length')"

    if [[ "${running}" -ge 1 && "${terminating}" -eq 0 ]]; then
      return 0
    fi

    if (( $(date +%s) - start_ts > timeout_sec )); then
      echo "Gateway pods did not reach stable state in ${timeout_sec}s (running=${running}, terminating=${terminating})"
      kubectl get pods -n edgion-system -l app=edgion-gateway -o wide || true
      return 1
    fi

    sleep 2
  done
}

override_suite_test_server_images() {
  if [[ -z "${TEST_SERVER_IMAGE}" ]]; then
    return 0
  fi

  local ns
  for ns in edgion-default edgion-test edgion-backend default; do
    if kubectl get deployment lb-multi-test-server -n "${ns}" >/dev/null 2>&1; then
      echo "Override lb-multi-test-server image in ${ns}: ${TEST_SERVER_IMAGE}"
      kubectl set image deployment/lb-multi-test-server -n "${ns}" test-server="${TEST_SERVER_IMAGE}"
      kubectl rollout status deployment/lb-multi-test-server -n "${ns}" --timeout=300s
    fi
  done
}

restart_secret_dependent_workloads() {
  if kubectl get deployment edgion-test-server -n edgion-test >/dev/null 2>&1; then
    echo "Restarting edgion-test/edgion-test-server to reload runtime TLS certs..."
    kubectl rollout restart deployment/edgion-test-server -n edgion-test
    kubectl rollout status deployment/edgion-test-server -n edgion-test --timeout=300s
  fi
}

get_server_id() {
  local admin_url="$1"
  local sid
  sid="$(curl -fsS "${admin_url}/api/v1/server-info" 2>/dev/null | jq -r '.server_id // "unknown"' || true)"
  if [[ -z "${sid}" ]]; then
    sid="unknown"
  fi
  echo "${sid}"
}

trigger_reload() {
  local admin_url="$1"
  local resp
  resp="$(curl -fsS -X POST "${admin_url}/api/v1/reload" 2>/dev/null || true)"
  if [[ -z "${resp}" ]]; then
    echo "reload api returned empty response: ${admin_url}/api/v1/reload"
    return 1
  fi
  if ! echo "${resp}" | jq -e '.success == true' >/dev/null 2>&1; then
    echo "reload api failed: ${resp}"
    return 1
  fi
  return 0
}

is_slow_test() {
  local test_name="$1"
  for slow in "${SLOW_TESTS[@]}"; do
    if [[ "${test_name}" == "${slow}" ]]; then
      return 0
    fi
  done
  return 1
}

suite_test_name() {
  local resource="$1"
  local item="$2"
  local safe_item
  safe_item="$(echo "${item}" | tr '/' '_')"
  echo "${resource}_${safe_item}"
}

suite_matches_filter() {
  local resource="$1"
  local item="$2"

  if [[ -z "${ONLY_RESOURCE}" ]]; then
    return 0
  fi
  if [[ "${ONLY_RESOURCE}" != "${resource}" ]]; then
    return 1
  fi
  if [[ -z "${ONLY_ITEM}" ]]; then
    return 0
  fi
  [[ "${ONLY_ITEM}" == "${item}" ]]
}

run_one() {
  local suite_dir="$1"
  local resource="$2"
  local item="$3"
  local suite_conf_dir="${CONF_ROOT}/${suite_dir}"

  if ! suite_matches_filter "${resource}" "${item}"; then
    echo "Skip (filtered): ${suite_dir}"
    return 0
  fi

  SELECTED_COUNT=$((SELECTED_COUNT + 1))

  if [[ ! -d "${suite_conf_dir}" ]]; then
    echo "Skip suite: ${suite_dir} (missing config dir: ${suite_conf_dir})"
    MISSING_COUNT=$((MISSING_COUNT + 1))
    MISSING_SUITES+=("${suite_dir}")
    return 0
  fi

  local tname
  tname="$(suite_test_name "${resource}" "${item}")"
  if [[ "${FULL_TEST}" != "true" ]] && is_slow_test "${tname}"; then
    echo "Skip slow suite (use --full-test): ${suite_dir}"
    return 0
  fi

  echo
  echo "=============================="
  echo "Running suite: ${suite_dir} (resource=${resource}, item=${item})"
  echo "=============================="

  EXECUTED_COUNT=$((EXECUTED_COUNT + 1))
  local max_attempts=2
  if [[ "${resource}" == "EdgionPlugins" && "${item}" == "DebugAccessLog" ]]; then
    max_attempts=6
  fi

  local attempt=1
  while (( attempt <= max_attempts )); do
    if "${RUN_CLIENT_SCRIPT}" -r "${resource}" -i "${item}"; then
      return 0
    fi

    if (( attempt == max_attempts )); then
      return 1
    fi

    echo "Run failed for ${resource}/${item}, retry #$((attempt + 1))/${max_attempts}..."
    sleep 3
    attempt=$((attempt + 1))
  done
}

run_all_selected_suites() {
  local use_start_from="$1"

  # Map: config suite dir|resource|item
  local suites=(
    "HTTPRoute/Basic|HTTPRoute|Basic"
    "HTTPRoute/Match|HTTPRoute|Match"
    "HTTPRoute/Backend/LBRoundRobin|HTTPRoute|Backend/LBRoundRobin"
    "HTTPRoute/Backend/LBConsistentHash|HTTPRoute|Backend/LBConsistentHash"
    "HTTPRoute/Backend/WeightedBackend|HTTPRoute|Backend/WeightedBackend"
    "HTTPRoute/Backend/Timeout|HTTPRoute|Backend/Timeout"
    "HTTPRoute/Filters/Redirect|HTTPRoute|Filters/Redirect"
    "HTTPRoute/Filters/Security|HTTPRoute|Filters/Security"
    "HTTPRoute/Filters/HeaderModifier|HTTPRoute|Filters/HeaderModifier"
    "HTTPRoute/Protocol/WebSocket|HTTPRoute|Protocol/WebSocket"
    "GRPCRoute/Basic|GRPCRoute|Basic"
    "GRPCRoute/Match|GRPCRoute|Match"
    "TCPRoute/Basic|TCPRoute|Basic"
    "TCPRoute/StreamPlugins|TCPRoute|StreamPlugins"
    "UDPRoute/Basic|UDPRoute|Basic"
    "Gateway/Security|Gateway|Security"
    "Gateway/RealIP|Gateway|RealIP"
    "Gateway/TLS/BackendTLS|Gateway|TLS/BackendTLS"
    "Gateway/TLS/GatewayTLS|Gateway|TLS/GatewayTLS"
    "Gateway/ListenerHostname|Gateway|ListenerHostname"
    "Gateway/AllowedRoutes/Same|Gateway|AllowedRoutes/Same"
    "Gateway/AllowedRoutes/All|Gateway|AllowedRoutes/All"
    "Gateway/AllowedRoutes/Kinds|Gateway|AllowedRoutes/Kinds"
    "Gateway/Combined|Gateway|Combined"
    "Gateway/StreamPlugins|Gateway|StreamPlugins"
    "Gateway/PortConflict|Gateway|PortConflict"
    "EdgionTls/https|EdgionTls|https"
    "EdgionTls/grpctls|EdgionTls|grpctls"
    "EdgionTls/mTLS|EdgionTls|mTLS"
    "EdgionTls/cipher|EdgionTls|cipher"
    "EdgionPlugins/DebugAccessLog|EdgionPlugins|DebugAccessLog"
    "EdgionPlugins/PluginCondition|EdgionPlugins|PluginCondition"
    "EdgionPlugins/PluginCondition/AllConditions|EdgionPlugins|PluginCondition/AllConditions"
    "EdgionPlugins/CtxSet|EdgionPlugins|CtxSet"
    "EdgionPlugins/JwtAuth|EdgionPlugins|JwtAuth"
    "EdgionPlugins/JweDecrypt|EdgionPlugins|JweDecrypt"
    "EdgionPlugins/HmacAuth|EdgionPlugins|HmacAuth"
    "EdgionPlugins/HeaderCertAuth|EdgionPlugins|HeaderCertAuth"
    "EdgionPlugins/KeyAuth|EdgionPlugins|KeyAuth"
    "EdgionPlugins/BasicAuth|EdgionPlugins|BasicAuth"
    "EdgionPlugins/ProxyRewrite|EdgionPlugins|ProxyRewrite"
    "EdgionPlugins/RateLimit|EdgionPlugins|RateLimit"
    "EdgionPlugins/RealIp|EdgionPlugins|RealIp"
    "EdgionPlugins/ResponseRewrite|EdgionPlugins|ResponseRewrite"
    "EdgionPlugins/RequestRestriction|EdgionPlugins|RequestRestriction"
    "EdgionPlugins/ForwardAuth|EdgionPlugins|ForwardAuth"
    "EdgionPlugins/OpenidConnect|EdgionPlugins|OpenidConnect"
    "EdgionPlugins/BandwidthLimit|EdgionPlugins|BandwidthLimit"
    "EdgionPlugins/DirectEndpoint|EdgionPlugins|DirectEndpoint"
    "EdgionPlugins/DynamicInternalUpstream|EdgionPlugins|DynamicInternalUpstream"
    "EdgionPlugins/DynamicExternalUpstream|EdgionPlugins|DynamicExternalUpstream"
    "EdgionPlugins/WebhookKeyGet|EdgionPlugins|WebhookKeyGet"
    "EdgionPlugins/Dsl|EdgionPlugins|Dsl"
    "EdgionPlugins/AllEndpointStatus|EdgionPlugins|AllEndpointStatus"
    "EdgionPlugins/LdapAuth|EdgionPlugins|LdapAuth"
  )

  local local_start_from="${START_FROM}"
  if [[ "${use_start_from}" != "true" ]]; then
    local_start_from=""
  fi

  for pair in "${suites[@]}"; do
    local suite_dir rest resource item suite_key
    suite_dir="${pair%%|*}"
    rest="${pair#*|}"
    resource="${rest%%|*}"
    item="${rest#*|}"
    suite_key="${resource}/${item}"

    if [[ -n "${local_start_from}" ]]; then
      if [[ "${suite_dir}" != "${local_start_from}" && "${suite_key}" != "${local_start_from}" ]]; then
        echo "Skip (before START_FROM): ${suite_dir}"
        continue
      fi
      local_start_from=""
    fi

    run_one "${suite_dir}" "${resource}" "${item}"
  done
}

if [[ "${SKIP_PREPARE}" == "false" ]]; then
  echo "Phase 1/2: Prepare environment"
  echo "Using BACKEND_TEST_NAMESPACE=${BACKEND_TEST_NAMESPACE}"
  print_cluster_and_image_context
  PREPARE_CONF_HASH="$(compute_prepare_conf_hash)"
  "${VALIDATE_SCRIPT}" "${CONF_ROOT}"

  if [[ "${SKIP_DEPLOY}" == "false" ]]; then
    # Ensure namespaces exist before applying generated secrets.
    if [[ -f "${K8S_DEPLOY_ROOT}/namespace.yaml" ]]; then
      kubectl apply -f "${K8S_DEPLOY_ROOT}/namespace.yaml"
    fi
  fi

  echo "Generate runtime TLS/mTLS secrets..."
  "${GENERATE_CERTS_SCRIPT}" "${GENERATED_DIR}"
  if [[ ! -d "${GENERATED_SECRET_DIR}" ]]; then
    echo "generated secret dir not found: ${GENERATED_SECRET_DIR}"
    exit 1
  fi
  kubectl apply --server-side --force-conflicts --field-manager=edgion-k8s-test -f "${GENERATED_SECRET_DIR}"

  if [[ "${SKIP_DEPLOY}" == "false" ]]; then
    export CONTROLLER_IMAGE GATEWAY_IMAGE TEST_CLIENT_IMAGE TEST_SERVER_IMAGE
    K8S_DEPLOY_ROOT="$K8S_DEPLOY_ROOT" BACKEND_TEST_NAMESPACE="${BACKEND_TEST_NAMESPACE}" "${DEPLOY_SCRIPT}" \
      --test-server-replicas "${TEST_SERVER_REPLICAS}"
  fi

  "${APPLY_ALL_SCRIPT}" "${CONF_ROOT}"
  override_suite_test_server_images
  restart_secret_dependent_workloads

  echo "Restarting gateway after full apply..."
  kubectl rollout restart deployment/edgion-gateway -n edgion-system
  kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=300s
  wait_gateway_stable 300
  sleep 5

  write_prepare_state_marker "${PREPARE_CONF_HASH}"
  echo "Prepare phase finished."
else
  print_cluster_and_image_context
  verify_skip_prepare_preconditions
fi

if [[ "${PREPARE_ONLY}" == "true" ]]; then
  echo "Prepare-only mode finished. Cluster state is kept."
  exit 0
fi

echo "Phase 2/2: Run tests"
if [[ "${FULL_TEST}" == "true" ]]; then
  echo "Mode: Full test (includes slow suites)"
else
  echo "Mode: Fast test (slow suites skipped; use --full-test to include)"
fi
if [[ "${WITH_RELOAD}" == "true" ]]; then
  echo "Rounds: 2 (with reload)"
  c_before="$(get_server_id "${CONTROLLER_ADMIN_URL}")"
  g_before="$(get_server_id "${GATEWAY_ADMIN_URL}")"
  echo "Before reload: controller=${c_before} gateway=${g_before}"

  echo "Test round #1"
  run_all_selected_suites "true"

  echo "Triggering reload..."
  trigger_reload "${CONTROLLER_ADMIN_URL}"
  sleep 3
  wait_gateway_stable 120

  c_after="$(get_server_id "${CONTROLLER_ADMIN_URL}")"
  g_after="$(get_server_id "${GATEWAY_ADMIN_URL}")"
  echo "After reload: controller=${c_after} gateway=${g_after}"

  if [[ "${c_before}" == "${c_after}" || "${g_before}" == "${g_after}" ]]; then
    echo "reload check failed: server_id not changed as expected"
    exit 1
  fi

  echo "Test round #2"
  run_all_selected_suites "false"
else
  echo "Rounds: 1"
  run_all_selected_suites "true"
fi

echo
if [[ "${FILTERED_MODE}" == "true" ]]; then
  if [[ "${SELECTED_COUNT}" -eq 0 ]]; then
    echo "No suite matched filter: resource=${ONLY_RESOURCE} item=${ONLY_ITEM:-<all>}"
    exit 1
  fi
  if [[ "${MISSING_COUNT}" -gt 0 ]]; then
    echo "Filtered run has missing suite config:"
    for s in "${MISSING_SUITES[@]}"; do
      echo "  - ${s}"
    done
    exit 1
  fi
fi

if [[ "${MISSING_COUNT}" -gt 0 ]]; then
  echo "Warning: skipped ${MISSING_COUNT} suites due to missing k8s conf dirs."
fi

echo "K8s integration run finished. Cluster state is kept by default."
