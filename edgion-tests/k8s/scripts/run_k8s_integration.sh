#!/usr/bin/env bash
set -euo pipefail

# Track background port-forward PIDs so they are always cleaned up, including on
# SIGINT/early abort (the script uses `set -e`, so a fall-through-only kill would
# leak the forward on interrupt).
_PF_PIDS=()
trap 'for p in "${_PF_PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done' EXIT INT TERM

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/conf}"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/kubernetes}"
COMMON_SCRIPT_DIR="${SCRIPT_DIR}/common"
ORBSTACK_SCRIPT_DIR="${SCRIPT_DIR}/orbstack"

VALIDATE_SCRIPT="$COMMON_SCRIPT_DIR/validate_no_endpoints.sh"
DEPLOY_SCRIPT="$COMMON_SCRIPT_DIR/deploy_integration.sh"
APPLY_ALL_SCRIPT="$COMMON_SCRIPT_DIR/apply_all_conf_strict.sh"
RUN_CLIENT_SCRIPT="$COMMON_SCRIPT_DIR/run_test_client.sh"
RUN_CLIENT_BATCH_SCRIPT="$COMMON_SCRIPT_DIR/run_test_client_batch.sh"
GENERATE_CERTS_SCRIPT="$COMMON_SCRIPT_DIR/generate_runtime_certs.sh"
LOCAL_IMAGE_PREPARE_SCRIPT="$ORBSTACK_SCRIPT_DIR/prepare_local_images.sh"
GENERATED_DIR="${GENERATED_DIR:-$PROJECT_ROOT/edgion-tests/k8s/generated}"
GENERATED_SECRET_DIR="${GENERATED_DIR}/secrets"
IMAGE_CONFIG_FILE="${IMAGE_CONFIG_FILE:-$SCRIPT_DIR/conf.env}"
STATE_NAMESPACE="${STATE_NAMESPACE:-edgion-system}"
STATE_CONFIGMAP_NAME="${STATE_CONFIGMAP_NAME:-edgion-k8s-integration-state}"

CONTROLLER_ADMIN_URL="${EDGION_CONTROLLER_ADMIN_URL:-http://edgion-controller.edgion-system.svc.cluster.local:12101}"

# Gateway admin API (port 12001) requires a Bearer token. The gateway Deployment reads
# it from the edgion-gateway-admin-credentials Secret (key: token) via secretKeyRef, and
# this harness sends the same value when curling the gateway admin API through a
# port-forward. Define the token once here so the Secret and the curls always match.
GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-integration-gateway-admin-token-0123456789}"
export EDGION_GATEWAY_ADMIN_TOKEN="${GATEWAY_ADMIN_TOKEN}"
CONTROLLER_ADMIN_TOKEN="${EDGION_TEST_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
export EDGION_TEST_ADMIN_TOKEN="${CONTROLLER_ADMIN_TOKEN}"
export EDGION_TOKEN="${EDGION_TOKEN:-${CONTROLLER_ADMIN_TOKEN}}"

START_FROM="${START_FROM:-}"
ONLY_RESOURCE=""
ONLY_ITEM=""
SKIP_DEPLOY=false
SKIP_PREPARE=false
FORCE_PREPARE=false
PREPARE_ONLY=false
TEST_SERVER_REPLICAS="${TEST_SERVER_REPLICAS:-3}"
SPEC_PROFILE="${SPEC_PROFILE:-}"
FULL_TEST=false
WITH_RELOAD=false
BACKEND_TEST_NAMESPACE="${BACKEND_TEST_NAMESPACE:-edgion-backend}"
CLUSTER_MODE="${CLUSTER_MODE:-auto}" # auto|local|remote
LOCAL_IMAGE_MODE="${LOCAL_IMAGE_MODE:-auto}" # auto|remote-pull|build-import|prepare-local-images
TEST_CLIENT_IMAGE="${TEST_CLIENT_IMAGE:-}"
TEST_SERVER_IMAGE="${TEST_SERVER_IMAGE:-}"
CONTROLLER_IMAGE="${CONTROLLER_IMAGE:-}"
GATEWAY_IMAGE="${GATEWAY_IMAGE:-}"
IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
IMAGE_VERSION="${IMAGE_VERSION:-0.3.2}"
IMAGE_ARCH="${IMAGE_ARCH:-}"
PREPARE_LOCAL_IMAGES=false
LOCAL_IMAGE_REBUILD=false
PULL_POLICY_OVERRIDE="${PULL_POLICY_OVERRIDE:-}"
LOCAL_IMAGE_MODE_RESOLVED=""
LOCAL_BUILD_IMAGE_VERSION_SUFFIX="${LOCAL_BUILD_IMAGE_VERSION_SUFFIX:-}"
LOCAL_TEST_BIN_DIR="${LOCAL_TEST_BIN_DIR:-}"
LOCAL_CORE_BIN_DIR="${LOCAL_CORE_BIN_DIR:-}"
LOCAL_TEST_RUNTIME_IMAGE="${LOCAL_TEST_RUNTIME_IMAGE:-ubuntu:24.04}"

if [[ -z "${LOCAL_CORE_BIN_DIR}" && -n "${LOCAL_TEST_BIN_DIR}" \
  && -x "${LOCAL_TEST_BIN_DIR}/edgion-controller" \
  && -x "${LOCAL_TEST_BIN_DIR}/edgion-gateway" ]]; then
  LOCAL_CORE_BIN_DIR="${LOCAL_TEST_BIN_DIR}"
fi

FILTERED_MODE=false
SELECTED_COUNT=0
EXECUTED_COUNT=0
MISSING_COUNT=0
FAIL_COUNT=0
MISSING_SUITES=()
FAILED_SUITES=()

SLOW_TESTS=(
  "HTTPRoute_Backend_Timeout"
  "HTTPRoute_Backend_HealthCheckTransition"
  "HTTPRoute_Backend_PerTryTimeout"
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
  --force-prepare                Force prepare even if state ConfigMap is up-to-date
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
  --local-image-mode <auto|remote-pull|build-import|prepare-local-images>
                                 Local cluster image mode (default: ${LOCAL_IMAGE_MODE})
  --image-config <path>          Image config file (default: scripts/conf.env)
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
  LOCAL_IMAGE_MODE              Same as --local-image-mode
  LOCAL_TEST_BIN_DIR             Directory containing linux/amd64 test_client and test_server.
                                 When set, test pods use LOCAL_TEST_RUNTIME_IMAGE
                                 and binaries are copied into running pods.
  LOCAL_TEST_RUNTIME_IMAGE       Runtime image for LOCAL_TEST_BIN_DIR mode
                                 (default: ${LOCAL_TEST_RUNTIME_IMAGE})
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
    --force-prepare)
      FORCE_PREPARE=true
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
    --local-image-mode)
      LOCAL_IMAGE_MODE="${2:-}"
      if [[ -z "${LOCAL_IMAGE_MODE}" ]]; then
        echo "missing value for --local-image-mode"
        exit 1
      fi
      if [[ "${LOCAL_IMAGE_MODE}" != "auto" && "${LOCAL_IMAGE_MODE}" != "remote-pull" \
         && "${LOCAL_IMAGE_MODE}" != "build-import" && "${LOCAL_IMAGE_MODE}" != "prepare-local-images" ]]; then
        echo "invalid --local-image-mode: ${LOCAL_IMAGE_MODE} (expected: auto|remote-pull|build-import|prepare-local-images)"
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

for required in "$VALIDATE_SCRIPT" "$DEPLOY_SCRIPT" "$APPLY_ALL_SCRIPT" "$RUN_CLIENT_SCRIPT" "$RUN_CLIENT_BATCH_SCRIPT" "$GENERATE_CERTS_SCRIPT"; do
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

get_cluster_server() {
  kubectl config view --minify -o jsonpath='{.clusters[0].cluster.server}' 2>/dev/null || true
}

detect_cluster_mode_auto() {
  local names server
  server="$(get_cluster_server | tr '[:upper:]' '[:lower:]')"
  if [[ "${server}" == *"127.0.0.1"* || "${server}" == *"localhost"* ]]; then
    echo "local"
    return 0
  fi

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

  # Preserve auto-detected IMAGE_ARCH; only let the config file override
  # registry/namespace/version and per-image vars that are still empty.
  local _saved_arch="${IMAGE_ARCH:-}"

  # shellcheck disable=SC1090
  source "${f}"

  # Restore auto-detected arch when the config file would downgrade it.
  if [[ -n "${_saved_arch}" ]]; then
    IMAGE_ARCH="${_saved_arch}"
  fi

  IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
  IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
  IMAGE_VERSION="${IMAGE_VERSION:-0.3.2}"

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

auto_detect_image_arch() {
  local primary
  primary="$(get_cluster_arches | head -n1)"
  case "${primary}" in
    arm64|aarch64) echo "arm64" ;;
    amd64|x86_64)  echo "amd64" ;;
    *)             echo "amd64" ;;
  esac
}

prompt_local_image_mode() {
  local selected
  if [[ "${LOCAL_IMAGE_MODE}" != "auto" ]]; then
    echo "${LOCAL_IMAGE_MODE}"
    return 0
  fi

  if [[ ! -t 0 ]]; then
    echo "remote-pull"
    return 0
  fi

  cat >&2 <<EOF
Local cluster detected. Choose image mode:
  1) remote-pull          (default) use imagePullPolicy=Always and pull from registry
  2) build-import         build local images, docker save, import into local runtime, use IfNotPresent
  3) prepare-local-images run existing --prepare-local-images flow, use IfNotPresent
EOF
  read -r -p "Select [1/2/3] (default 1): " selected >&2
  case "${selected:-1}" in
    2) echo "build-import" ;;
    3) echo "prepare-local-images" ;;
    *) echo "remote-pull" ;;
  esac
}

build_and_import_local_images() {
  local arch build_script build_version
  build_script="${PROJECT_ROOT}/cicd/build-image.sh"
  arch="${IMAGE_ARCH}"
  if [[ -z "${arch}" ]]; then
    arch="$(auto_detect_image_arch)"
  fi

  if [[ -n "${LOCAL_BUILD_IMAGE_VERSION_SUFFIX}" ]]; then
    build_version="${IMAGE_VERSION}-${LOCAL_BUILD_IMAGE_VERSION_SUFFIX}"
  else
    build_version="${IMAGE_VERSION}-local$(date +%Y%m%d%H%M%S)"
  fi
  echo "[local build-import] resolved image version: ${build_version}"

  # Standard images (controller + gateway) and test harness images
  # (test_server + test_client) are now separate build modes.
  local build_args=(--arch "${arch}" --version "${build_version}")
  if [[ "${LOCAL_IMAGE_REBUILD}" == "true" ]]; then
    build_args+=(--rebuild)
  fi

  if [[ ! -x "${build_script}" ]]; then
    echo "build script not found or not executable: ${build_script}"
    exit 1
  fi

  echo "[local build-import] building local standard images: ${build_args[*]}"
  IMAGE_REGISTRY="${IMAGE_REGISTRY}" IMAGE_NAMESPACE="${IMAGE_NAMESPACE}" "${build_script}" "${build_args[@]}"

  echo "[local build-import] building local test images: ${build_args[*]} --test-images"
  IMAGE_REGISTRY="${IMAGE_REGISTRY}" IMAGE_NAMESPACE="${IMAGE_NAMESPACE}" "${build_script}" "${build_args[@]}" --test-images

  IMAGE_VERSION="${build_version}"
  CONTROLLER_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-controller:${IMAGE_VERSION}_${arch}"
  GATEWAY_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-gateway:${IMAGE_VERSION}_${arch}"
  TEST_CLIENT_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-test-client:${IMAGE_VERSION}_${arch}"
  TEST_SERVER_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-test-server:${IMAGE_VERSION}_${arch}"

  local tar_file
  tar_file="$(mktemp /tmp/edgion-k8s-images.XXXXXX.tar)"
  echo "[local build-import] saving images to ${tar_file}"
  docker save \
    "${CONTROLLER_IMAGE}" \
    "${GATEWAY_IMAGE}" \
    "${TEST_CLIENT_IMAGE}" \
    "${TEST_SERVER_IMAGE}" \
    -o "${tar_file}"

  local imported=false
  if command -v ctr >/dev/null 2>&1; then
    echo "[local build-import] importing into containerd (ctr -n k8s.io images import)"
    ctr -n k8s.io images import "${tar_file}"
    imported=true
  elif command -v nerdctl >/dev/null 2>&1; then
    echo "[local build-import] importing via nerdctl -n k8s.io load"
    nerdctl -n k8s.io load -i "${tar_file}"
    imported=true
  elif command -v kind >/dev/null 2>&1 && kubectl config current-context 2>/dev/null | grep -qi '^kind-'; then
    echo "[local build-import] importing via kind load docker-image"
    kind load docker-image "${CONTROLLER_IMAGE}" "${GATEWAY_IMAGE}" "${TEST_CLIENT_IMAGE}" "${TEST_SERVER_IMAGE}"
    imported=true
  fi

  if [[ "${imported}" != "true" ]]; then
    echo "[WARN] no runtime import tool detected (ctr/nerdctl/kind). Assuming cluster can use local docker images."
  fi

  rm -f "${tar_file}"
}

local_test_binary_mode() {
  [[ -n "${LOCAL_TEST_BIN_DIR}" ]]
}

local_core_binary_mode() {
  [[ -n "${LOCAL_CORE_BIN_DIR}" ]]
}

require_local_test_binaries() {
  if ! local_test_binary_mode; then
    return 0
  fi

  for bin in test_client test_server; do
    if [[ ! -x "${LOCAL_TEST_BIN_DIR}/${bin}" ]]; then
      echo "LOCAL_TEST_BIN_DIR is set but executable not found: ${LOCAL_TEST_BIN_DIR}/${bin}"
      exit 1
    fi
  done
}

require_local_core_binaries() {
  if ! local_core_binary_mode; then
    return 0
  fi

  for bin in edgion-controller edgion-gateway; do
    if [[ ! -x "${LOCAL_CORE_BIN_DIR}/${bin}" ]]; then
      echo "LOCAL_CORE_BIN_DIR is set but executable not found: ${LOCAL_CORE_BIN_DIR}/${bin}"
      exit 1
    fi
  done
}

apply_local_test_image_overrides() {
  if ! local_test_binary_mode; then
    return 0
  fi

  TEST_CLIENT_IMAGE="${LOCAL_TEST_RUNTIME_IMAGE}"
  TEST_SERVER_IMAGE="${LOCAL_TEST_RUNTIME_IMAGE}"
  PULL_POLICY_OVERRIDE="IfNotPresent"
  echo "Local test binary mode enabled: ${LOCAL_TEST_BIN_DIR}"
  echo "Test pods will use runtime image: ${LOCAL_TEST_RUNTIME_IMAGE}"
}

print_cluster_and_image_context() {
  local mode arches primary local_mode
  mode="$(detect_cluster_mode)"
  arches="$(get_cluster_arches | xargs)"
  primary="$(get_cluster_arches | head -n1)"

  # Auto-detect IMAGE_ARCH from cluster nodes when not explicitly set.
  if [[ -z "${IMAGE_ARCH}" ]]; then
    IMAGE_ARCH="$(auto_detect_image_arch)"
    echo "Auto-detected IMAGE_ARCH=${IMAGE_ARCH} from cluster node arch (${primary})"
  fi

  load_image_config_file "${IMAGE_CONFIG_FILE}"

  if [[ "${mode}" == "local" ]]; then
    local_mode="$(prompt_local_image_mode)"
    LOCAL_IMAGE_MODE_RESOLVED="${local_mode}"
    echo "Local image mode: ${local_mode}"
    case "${local_mode}" in
      remote-pull)
        PULL_POLICY_OVERRIDE=""
        ;;
      build-import)
        build_and_import_local_images
        PULL_POLICY_OVERRIDE="IfNotPresent"
        ;;
      prepare-local-images)
        PREPARE_LOCAL_IMAGES=true
        PULL_POLICY_OVERRIDE="IfNotPresent"
        ;;
      *)
        echo "invalid local image mode: ${local_mode}"
        exit 1
        ;;
    esac
  else
    LOCAL_IMAGE_MODE_RESOLVED="remote-pull"
  fi

  prepare_local_images_if_needed "${mode}"
  apply_local_test_image_overrides

  echo "Cluster mode: ${mode} (configured: ${CLUSTER_MODE})"
  echo "Cluster node arch(es): ${arches}"
  if local_core_binary_mode; then
    echo "Local core binary mode enabled: ${LOCAL_CORE_BIN_DIR}"
  fi
  echo "Image override CONTROLLER_IMAGE: ${CONTROLLER_IMAGE:-<unset>}"
  echo "Image override GATEWAY_IMAGE: ${GATEWAY_IMAGE:-<unset>}"
  echo "Image override TEST_CLIENT_IMAGE: ${TEST_CLIENT_IMAGE:-<unset>}"
  echo "Image override TEST_SERVER_IMAGE: ${TEST_SERVER_IMAGE:-<unset>}"
  echo "Pull policy override: ${PULL_POLICY_OVERRIDE:-<none>}"

  check_image_arch_compat "${CONTROLLER_IMAGE}" "${primary}" "CONTROLLER_IMAGE"
  check_image_arch_compat "${GATEWAY_IMAGE}" "${primary}" "GATEWAY_IMAGE"
  check_image_arch_compat "${TEST_CLIENT_IMAGE}" "${primary}" "TEST_CLIENT_IMAGE"
  check_image_arch_compat "${TEST_SERVER_IMAGE}" "${primary}" "TEST_SERVER_IMAGE"

  if [[ "${mode}" == "remote" && "${SKIP_DEPLOY}" == "false" ]]; then
    if [[ -z "${CONTROLLER_IMAGE}" || -z "${GATEWAY_IMAGE}" || -z "${TEST_CLIENT_IMAGE}" || -z "${TEST_SERVER_IMAGE}" ]]; then
      echo "[WARN] remote mode detected but some images are not set."
      echo "[WARN] IMAGE_ARCH=${IMAGE_ARCH} should auto-resolve them; check images-remote.env or set explicitly."
    fi
  fi
}

apply_all_conf_with_fallback() {
  if "${APPLY_ALL_SCRIPT}" "${CONF_ROOT}"; then
    return 0
  fi
  local strict_rc=$?

  # `apply_all_conf_strict.sh` depends on PyYAML; in minimal environments
  # provide a best-effort server-side apply fallback instead of hard-failing.
  if python3 -c 'import yaml' >/dev/null 2>&1; then
    return "${strict_rc}"
  fi

  echo "[WARN] apply_all_conf_strict failed and python3-yaml is unavailable."
  echo "[WARN] Falling back to sorted recursive server-side apply under: ${CONF_ROOT}"
  local exclude_re
  exclude_re='00-namespace\.ya?ml$|01-deployment\.ya?ml$|/Gateway/DynamicTest/(updates|delete)/|/base/Secret_edgion-test_edge-tls\.ya?ml$|/EdgionTls/mTLS/Secret_edge_client-ca\.ya?ml$|/EdgionTls/mTLS/Secret_edge_ca-chain\.ya?ml$|/EdgionTls/mTLS/Secret_edge_mtls-server\.ya?ml$|/HTTPRoute/Backend/BackendTLS/Secret_backend-ca\.ya?ml$|/EdgionPlugins/HeaderCertAuth/01_Secret_default_header-cert-ca\.ya?ml$|/Gateway/PortConflict/Gateway_internal_conflict\.ya?ml$'
  local file
  while IFS= read -r file; do
    if echo "${file}" | grep -Eq "${exclude_re}"; then
      continue
    fi
    local ok=false attempt out rc saw_conflict=false
    for attempt in 1 2 3; do
      out="$(kubectl apply --server-side --force-conflicts --field-manager=edgion-k8s-test -f "${file}" 2>&1)" && rc=0 || rc=$?
      if [[ "${rc}" -eq 0 ]]; then
        echo "${out}"
        ok=true
        break
      fi
      if echo "${out}" | grep -qE 'Operation cannot be fulfilled|the object has been modified'; then
        saw_conflict=true
        echo "[WARN] transient apply conflict (${attempt}/3): ${file}"
        sleep 1
        continue
      fi
      echo "${out}"
      return "${rc}"
    done
    if [[ "${ok}" != "true" && "${saw_conflict}" == "true" ]]; then
      echo "[WARN] conflict persists, fallback to kubectl replace --force: ${file}"
      if kubectl replace --force -f "${file}"; then
        ok=true
      fi
    fi
    if [[ "${ok}" != "true" ]]; then
      echo "[ERROR] failed to apply after retries: ${file}"
      return 1
    fi
  done < <(find "${CONF_ROOT}" -type f \( -name "*.yaml" -o -name "*.yml" \) | sort)
}

ensure_gatewayclass_compat() {
  local gc_name expected current
  gc_name="public-gateway"
  expected="edgion.io/gateway-controller"
  current="$(kubectl get gatewayclass "${gc_name}" -o jsonpath='{.spec.controllerName}' 2>/dev/null || true)"

  if [[ -n "${current}" && "${current}" != "${expected}" ]]; then
    echo "[WARN] gatewayclass/${gc_name} controllerName mismatch: current=${current}, expected=${expected}"
    echo "[WARN] deleting gatewayclass/${gc_name} to allow recreation by test conf"
    kubectl delete gatewayclass "${gc_name}" --wait=true || true
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
  if [[ "${SKIP_DEPLOY}" == "true" ]]; then
    return 0
  fi

  if [[ -z "${TEST_SERVER_IMAGE}" ]]; then
    return 0
  fi

  local ns
  for ns in edgion-default edgion-test edgion-backend default; do
    if kubectl get deployment lb-multi-test-server -n "${ns}" >/dev/null 2>&1; then
      echo "Override lb-multi-test-server image in ${ns}: ${TEST_SERVER_IMAGE}"
      kubectl set image deployment/lb-multi-test-server -n "${ns}" test-server="${TEST_SERVER_IMAGE}"
      if [[ -n "${PULL_POLICY_OVERRIDE}" ]]; then
        kubectl patch deployment lb-multi-test-server -n "${ns}" --type='json' \
          -p="[{\"op\":\"replace\",\"path\":\"/spec/template/spec/containers/0/imagePullPolicy\",\"value\":\"${PULL_POLICY_OVERRIDE}\"}]" \
          >/dev/null 2>&1 || true
      fi
      kubectl rollout restart deployment/lb-multi-test-server -n "${ns}"
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

patch_local_binary_deployment() {
  local ns="$1"
  local name="$2"
  local container="$3"

  if ! kubectl get deployment "${name}" -n "${ns}" >/dev/null 2>&1; then
    return 0
  fi

  echo "Patch ${ns}/${name} for local test binary injection"
  kubectl patch deployment "${name}" -n "${ns}" --type='strategic' \
    -p="{\"spec\":{\"template\":{\"spec\":{\"containers\":[{\"name\":\"${container}\",\"image\":\"${LOCAL_TEST_RUNTIME_IMAGE}\",\"imagePullPolicy\":\"IfNotPresent\",\"command\":[\"sleep\",\"infinity\"],\"args\":[]}]}}}}"
  kubectl patch deployment "${name}" -n "${ns}" --type='json' \
    -p='[{"op":"remove","path":"/spec/template/spec/containers/0/livenessProbe"}]' >/dev/null 2>&1 || true
  kubectl patch deployment "${name}" -n "${ns}" --type='json' \
    -p='[{"op":"remove","path":"/spec/template/spec/containers/0/readinessProbe"}]' >/dev/null 2>&1 || true
}

patch_local_binary_test_deployments() {
  if ! local_test_binary_mode; then
    return 0
  fi

  patch_local_binary_deployment "edgion-test" "edgion-test-client" "test-client"
  patch_local_binary_deployment "edgion-test" "edgion-test-server" "test-server"
  patch_local_binary_deployment "edgion-default" "edgion-test-server" "test-server"
  patch_local_binary_deployment "${BACKEND_TEST_NAMESPACE}" "edgion-test-server" "test-server"
  patch_local_binary_deployment "edgion-default" "lb-multi-test-server" "test-server"
}

rollout_local_binary_test_deployments() {
  if ! local_test_binary_mode; then
    return 0
  fi

  local ns name
  for pair in \
    "edgion-test edgion-test-client" \
    "edgion-test edgion-test-server" \
    "edgion-default edgion-test-server" \
    "${BACKEND_TEST_NAMESPACE} edgion-test-server" \
    "edgion-default lb-multi-test-server"; do
    ns="${pair%% *}"
    name="${pair#* }"
    if kubectl get deployment "${name}" -n "${ns}" >/dev/null 2>&1; then
      kubectl rollout status deployment/"${name}" -n "${ns}" --timeout=300s
    fi
  done
}

label_single_backend_pod() {
  local pod
  kubectl label pod -n edgion-test -l app=edgion-test-server edgion.io/sticky-single- >/dev/null 2>&1 || true
  pod="$(
    kubectl get pods -n edgion-test -l app=edgion-test-server \
      --field-selector=status.phase=Running \
      --sort-by=.metadata.creationTimestamp \
      -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true
  )"
  if [[ -n "${pod}" ]]; then
    echo "Label single-state backend pod: edgion-test/${pod}"
    kubectl label pod -n edgion-test "${pod}" edgion.io/sticky-single=true --overwrite >/dev/null
  fi
}

kubectl_retry() {
  local attempt rc
  for attempt in 1 2 3; do
    if "$@"; then
      return 0
    fi
    rc=$?
    echo "[WARN] kubectl command failed (attempt ${attempt}/3, rc=${rc}); retrying..."
    sleep $((attempt * 2))
  done
  return "${rc}"
}

copy_local_binary_to_pods() {
  local ns="$1"
  local selector="$2"
  local container="$3"
  local binary="$4"
  local src="${LOCAL_TEST_BIN_DIR}/${binary}"
  local pods pod

  pods="$(kubectl get pods -n "${ns}" -l "${selector}" \
    --field-selector=status.phase=Running \
    -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}' 2>/dev/null || true)"
  if [[ -z "${pods}" ]]; then
    echo "No running pods found for ${ns}/${selector}"
    return 1
  fi

  for pod in ${pods}; do
    echo "Copy ${binary} to ${ns}/${pod}"
    kubectl_retry kubectl cp -n "${ns}" -c "${container}" "${src}" "${pod}:/usr/local/bin/${binary}"
    kubectl_retry kubectl exec -n "${ns}" -c "${container}" "${pod}" -- chmod +x "/usr/local/bin/${binary}"
  done
}

copy_local_core_binary_to_pods() {
  local ns="$1"
  local selector="$2"
  local container="$3"
  local binary="$4"
  local src="${LOCAL_CORE_BIN_DIR}/${binary}"
  local pods pod

  pods="$(kubectl get pods -n "${ns}" -l "${selector}" \
    --field-selector=status.phase=Running \
    -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}' 2>/dev/null || true)"
  if [[ -z "${pods}" ]]; then
    echo "No running pods found for ${ns}/${selector}"
    return 1
  fi

  for pod in ${pods}; do
    echo "Copy ${binary} to ${ns}/${pod}"
    kubectl_retry kubectl exec -n "${ns}" -c "${container}" "${pod}" -- sh -c 'mkdir -p /usr/local/edgion'
    kubectl_retry kubectl cp -n "${ns}" -c "${container}" "${src}" "${pod}:/tmp/${binary}.new"
    kubectl_retry kubectl exec -n "${ns}" -c "${container}" "${pod}" -- sh -c \
      "chmod +x /tmp/${binary}.new && mv /tmp/${binary}.new /usr/local/edgion/${binary}"
  done
}

restart_core_binary_in_pods() {
  local ns="$1"
  local selector="$2"
  local container="$3"
  local binary="$4"
  local pods pod

  pods="$(kubectl get pods -n "${ns}" -l "${selector}" \
    --field-selector=status.phase=Running \
    -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}' 2>/dev/null || true)"
  if [[ -z "${pods}" ]]; then
    echo "No running pods found for ${ns}/${selector}"
    return 1
  fi

  for pod in ${pods}; do
    echo "Restart ${binary} process in ${ns}/${pod}"
    kubectl exec -n "${ns}" -c "${container}" "${pod}" -- sh -c '
      bin="$1"
      pids=""
      if command -v pidof >/dev/null 2>&1; then
        pids="$(pidof "$bin" 2>/dev/null || true)"
      fi
      if [ -z "$pids" ] && command -v ps >/dev/null 2>&1; then
        pids="$(ps -eo pid=,comm= 2>/dev/null | while read -r pid comm _rest; do
          if [ "$comm" = "$bin" ]; then
            echo "$pid"
          fi
        done || true)"
      fi
      for p in $pids; do
        kill -TERM "$p" 2>/dev/null || true
      done
    ' sh "${binary}"
  done
}

install_local_core_binaries() {
  if ! local_core_binary_mode; then
    return 0
  fi

  require_local_core_binaries
  copy_local_core_binary_to_pods "edgion-system" "app=edgion-controller" "edgion-controller" "edgion-controller"
  copy_local_core_binary_to_pods "edgion-system" "app=edgion-gateway" "edgion-gateway" "edgion-gateway"
  restart_core_binary_in_pods "edgion-system" "app=edgion-controller" "edgion-controller" "edgion-controller"
  echo "Waiting for controller leadership after local binary restart..."
  sleep 30
  restart_core_binary_in_pods "edgion-system" "app=edgion-gateway" "edgion-gateway" "edgion-gateway"
  sleep 5
  wait_gateway_stable 300
  echo "Waiting for gateway listeners to settle after local binary restart..."
  sleep 15
}

install_local_test_client_tools() {
  local pods pod

  pods="$(kubectl get pods -n edgion-test -l app=edgion-test-client \
    --field-selector=status.phase=Running \
    -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}' 2>/dev/null || true)"
  if [[ -z "${pods}" ]]; then
    echo "No running test-client pods found for tool installation"
    return 1
  fi

  for pod in ${pods}; do
    echo "Ensure test-client tools in edgion-test/${pod}"
    kubectl exec -n edgion-test -c test-client "${pod}" -- sh -c \
      'if command -v openssl >/dev/null 2>&1 && command -v curl >/dev/null 2>&1; then exit 0; fi; apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends openssl curl ca-certificates procps && rm -rf /var/lib/apt/lists/*'
  done
}

start_local_test_server_pods() {
  local ns="$1"
  local selector="$2"
  local with_tls="${3:-false}"
  local pods pod args

  pods="$(kubectl get pods -n "${ns}" -l "${selector}" \
    --field-selector=status.phase=Running \
    -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}' 2>/dev/null || true)"
  if [[ -z "${pods}" ]]; then
    echo "No running test-server pods found for ${ns}/${selector}"
    return 1
  fi

  args="--http-ports=30001,30002,30003,30004 --grpc-ports=30021,30022 --websocket-port=30005 --tcp-port=30010 --tcp-pp2-port=30012 --udp-port=30011 --auth-port=30040 --ext-proc-port=30041 --log-level=info"
  if [[ "${with_tls}" == "true" ]]; then
    args="${args} --https-backend-port=30051 --https-backend-mtls-port=30052 --cert-file=/etc/edgion/backend-tls/tls.crt --key-file=/etc/edgion/backend-tls/tls.key --client-ca-file=/etc/edgion/client-ca/ca.crt --https-backend-multi-sni-port=30026 --multi-sni-cert-a=/etc/edgion/backend-sni-a/tls.crt --multi-sni-key-a=/etc/edgion/backend-sni-a/tls.key --multi-sni-cert-b=/etc/edgion/backend-sni-b/tls.crt --multi-sni-key-b=/etc/edgion/backend-sni-b/tls.key"
  fi

  for pod in ${pods}; do
    echo "Start test_server in ${ns}/${pod}"
    # Always (re)start so the freshly-copied binary is the one actually running.
    # The previous skip-if-running guard left a stale process serving an old
    # binary after copy_local_binary_to_pods refreshed the on-disk binary,
    # which silently broke any test relying on new test_server behavior.
    kubectl exec -n "${ns}" -c test-server "${pod}" -- sh -c \
      "if [ -f /tmp/test_server.pid ]; then kill \"\$(cat /tmp/test_server.pid)\" 2>/dev/null || true; fi; pkill -x test_server 2>/dev/null || true; sleep 1; nohup /usr/local/bin/test_server ${args} >/tmp/test_server.log 2>&1 & echo \$! >/tmp/test_server.pid; sleep 1; kill -0 \"\$(cat /tmp/test_server.pid)\" 2>/dev/null && echo \"test_server restarted (pid \$(cat /tmp/test_server.pid))\""
  done
}

install_local_test_binaries() {
  if ! local_test_binary_mode; then
    return 0
  fi

  require_local_test_binaries
  patch_local_binary_test_deployments
  rollout_local_binary_test_deployments
  label_single_backend_pod

  copy_local_binary_to_pods "edgion-test" "app=edgion-test-client" "test-client" "test_client"
  install_local_test_client_tools
  copy_local_binary_to_pods "edgion-test" "app=edgion-test-server" "test-server" "test_server"
  copy_local_binary_to_pods "edgion-default" "app=edgion-test-server" "test-server" "test_server"
  copy_local_binary_to_pods "${BACKEND_TEST_NAMESPACE}" "app=edgion-test-server" "test-server" "test_server"
  if kubectl get deployment lb-multi-test-server -n edgion-default >/dev/null 2>&1; then
    copy_local_binary_to_pods "edgion-default" "app=edgion-test-server-multi" "test-server" "test_server"
  fi

  start_local_test_server_pods "edgion-test" "app=edgion-test-server" "true"
  start_local_test_server_pods "edgion-default" "app=edgion-test-server" "false"
  start_local_test_server_pods "${BACKEND_TEST_NAMESPACE}" "app=edgion-test-server" "false"
  if kubectl get deployment lb-multi-test-server -n edgion-default >/dev/null 2>&1; then
    start_local_test_server_pods "edgion-default" "app=edgion-test-server-multi" "false"
  fi
}

sync_k8s_redis_auth_secret() {
  if ! kubectl get secret dragonfly-redis -n dragonfly-system >/dev/null 2>&1; then
    return 0
  fi

  echo "Syncing Dragonfly Redis auth Secret..."
  kubectl get secret dragonfly-redis -n dragonfly-system -o json \
    | jq '{
        apiVersion: "v1",
        kind: "Secret",
        metadata: {
          name: "redis-test-auth",
          namespace: "edgion-default"
        },
        type: "Opaque",
        data: {
          password: .data["redis-password"]
        }
      }' \
    | kubectl apply --server-side --force-conflicts \
        --field-manager=edgion-k8s-test -f -
}

# Work around init-phase race: Secret ResourceProcessor may finish after
# EdgionPlugins / EdgionTls / Gateway processors, so their resolved_* fields
# are empty. Annotating every Secret triggers a Watch UPDATE event which
# invokes SecretHandler.on_change -> cascading requeue for dependents.
force_sync_secrets() {
  local ts
  ts="$(date +%s)"
  echo "Force-syncing Secrets to fix init-phase ordering..."
  local ns secret_list
  for ns in edgion-default edgion-test backend cert-store; do
    secret_list="$(kubectl get secrets -n "${ns}" -o jsonpath='{.items[*].metadata.name}' 2>/dev/null || true)"
    for s in ${secret_list}; do
      kubectl annotate secret -n "${ns}" "${s}" \
        edgion.io/force-sync="${ts}" --overwrite >/dev/null 2>&1 || true
    done
  done
  echo "Waiting for controller to reprocess dependents..."
  sleep 10
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

# Fetch the Gateway's current server_id.
#
# The Gateway admin API (port 12001) is loopback-only and dropped from the Gateway
# Service, so it cannot be reached over Service DNS from the host. The Gateway image
# ships no curl/wget either, so a `kubectl exec ... curl` is not possible. Instead we
# open a short-lived port-forward to the Gateway Pod and curl it on 127.0.0.1.
# Gateway is replicas:1 and this suite is sequential. We request an ephemeral local
# port (`:12001`) and parse the chosen port from the port-forward output so a leaked
# forward from a prior run cannot cause a fixed-port collision. The PID is tracked in
# `_PF_PIDS` so the top-level trap reaps it even on interrupt.
get_gateway_server_id() {
  local gw_pod pf_pid sid local_port pf_log
  gw_pod="$(kubectl get pods -n edgion-system -l app=edgion-gateway \
    --field-selector=status.phase=Running \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
  if [[ -z "${gw_pod}" ]]; then
    echo "unknown"
    return 0
  fi

  pf_log="$(mktemp)"
  kubectl port-forward -n edgion-system "pod/${gw_pod}" :12001 >"${pf_log}" 2>&1 &
  pf_pid="$!"
  _PF_PIDS+=("${pf_pid}")

  # Discover the ephemeral local port kubectl picked, e.g.
  # "Forwarding from 127.0.0.1:54321 -> 12001".
  local attempt
  local_port=""
  for attempt in $(seq 1 20); do
    local_port="$(sed -n 's/.*Forwarding from 127\.0\.0\.1:\([0-9]\{1,\}\) ->.*/\1/p' "${pf_log}" 2>/dev/null | head -n 1)"
    [[ -n "${local_port}" ]] && break
    sleep 0.5
  done

  if [[ -z "${local_port}" ]]; then
    kill "${pf_pid}" >/dev/null 2>&1 || true
    wait "${pf_pid}" 2>/dev/null || true
    rm -f "${pf_log}"
    echo "unknown"
    return 0
  fi

  # Wait for the tunnel to come up (port accepting connections).
  for attempt in $(seq 1 20); do
    if curl -fsS -H "Authorization: Bearer ${GATEWAY_ADMIN_TOKEN}" \
      "http://127.0.0.1:${local_port}/api/v1/server-info" >/dev/null 2>&1; then
      break
    fi
    sleep 0.5
  done

  sid="$(curl -fsS -H "Authorization: Bearer ${GATEWAY_ADMIN_TOKEN}" \
    "http://127.0.0.1:${local_port}/api/v1/server-info" 2>/dev/null \
    | jq -r '.data.server_id // .server_id // "unknown"' || true)"

  kill "${pf_pid}" >/dev/null 2>&1 || true
  wait "${pf_pid}" 2>/dev/null || true
  rm -f "${pf_log}"

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
  local skip_count="${4:-false}"
  local suite_conf_dir="${CONF_ROOT}/${suite_dir}"

  if ! suite_matches_filter "${resource}" "${item}"; then
    echo "Skip (filtered): ${suite_dir}"
    return 0
  fi

  if [[ "${skip_count}" != "true" ]]; then
    SELECTED_COUNT=$((SELECTED_COUNT + 1))
  fi

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

  if [[ "${skip_count}" != "true" ]]; then
    EXECUTED_COUNT=$((EXECUTED_COUNT + 1))
  fi
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

is_host_side_suite() {
  local resource="$1" item="$2"
  [[ "${resource}" == "Gateway" && "${item}" == "PortConflict" ]]
}

run_all_selected_suites() {
  local use_start_from="$1"

  # Map: config suite dir|resource|item
  local suites=(
    "HTTPRoute/Basic|HTTPRoute|Basic"
    "HTTPRoute/Match|HTTPRoute|Match"
    "HTTPRoute/Backend/LBRoundRobin|HTTPRoute|Backend/LBRoundRobin"
    "HTTPRoute/Backend/LBConsistentHash|HTTPRoute|Backend/LBConsistentHash"
    "HTTPRoute/Backend/LBLeastConn|HTTPRoute|Backend/LBLeastConn"
    "HTTPRoute/Backend/LBLeastConnRetryOnStatus|HTTPRoute|Backend/LBLeastConnRetryOnStatus"
    "HTTPRoute/Backend/WeightedBackend|HTTPRoute|Backend/WeightedBackend"
    "HTTPRoute/Backend/HealthCheck|HTTPRoute|Backend/HealthCheck"
    "HTTPRoute/Backend/Timeout|HTTPRoute|Backend/Timeout"
    "HTTPRoute/Backend/HealthCheckTransition|HTTPRoute|Backend/HealthCheckTransition"
    "HTTPRoute/Backend/RetryOnStatus|HTTPRoute|Backend/RetryOnStatus"
    "HTTPRoute/Backend/PerTryTimeout|HTTPRoute|Backend/PerTryTimeout"
    "HTTPRoute/Backend/CircuitBreaker|HTTPRoute|Backend/CircuitBreaker"
    "HTTPRoute/Backend/SessionPersistence|HTTPRoute|Backend/SessionPersistence"
    "HTTPRoute/Backend/H2BackendMtlsPersistence|HTTPRoute|Backend/H2BackendMtlsPersistence"
    "HTTPRoute/Backend/BackendTLSSNIHostOverride|HTTPRoute|Backend/BackendTLSSNIHostOverride"
    "HTTPRoute/Backend/PartiallyInvalid|HTTPRoute|Backend/PartiallyInvalid"
    "HTTPRoute/Backend/GrpcHealthCheck|HTTPRoute|Backend/GrpcHealthCheck"
    "HTTPRoute/Backend/DnsResolutionFailure|HTTPRoute|Backend/DnsResolutionFailure"
    "HTTPRoute/Filters/Redirect|HTTPRoute|Filters/Redirect"
    "HTTPRoute/Filters/Security|HTTPRoute|Filters/Security"
    "HTTPRoute/Filters/HeaderModifier|HTTPRoute|Filters/HeaderModifier"
    "HTTPRoute/Filters/FilterConflict|HTTPRoute|Filters/FilterConflict"
    "HTTPRoute/Protocol/WebSocket|HTTPRoute|Protocol/WebSocket"
    "HTTPRoute/Protocol/H2C|HTTPRoute|Protocol/H2C"
    "HTTPRoute/Protocol/GrpcWeb|HTTPRoute|Protocol/GrpcWeb"
    "HTTPRoute/Protocol/H2TrailerMalformed|HTTPRoute|Protocol/H2TrailerMalformed"
    "HTTPRoute/MultiPort|HTTPRoute|MultiPort"
    "GRPCRoute/Basic|GRPCRoute|Basic"
    "GRPCRoute/Match|GRPCRoute|Match"
    "GRPCRoute/RateLimit|GRPCRoute|RateLimit"
    "TCPRoute/Basic|TCPRoute|Basic"
    "TCPRoute/StreamPlugins|TCPRoute|StreamPlugins"
    "TCPRoute/GlobalConnectionIpRestriction|TCPRoute|GlobalConnectionIpRestriction"
    "TCPRoute/IdleTimeout|TCPRoute|IdleTimeout"
    "TLSRoute/Basic|TLSRoute|Basic"
    "TLSRoute/ProxyProtocol|TLSRoute|ProxyProtocol"
    "TLSRoute/StreamPlugins|TLSRoute|StreamPlugins"
    "TLSRoute/MultiSNI|TLSRoute|MultiSNI"
    "TLSRoute/BothAbsentParentRef|TLSRoute|BothAbsentParentRef"
    "TLSRoute/Passthrough|TLSRoute|Passthrough"
    "UDPRoute/Basic|UDPRoute|Basic"
    "UDPRoute/StreamPlugins|UDPRoute|StreamPlugins"
    "Gateway/Security|Gateway|Security"
    "Gateway/RealIP|Gateway|RealIP"
    "Gateway/TLS/BackendTLS|Gateway|TLS/BackendTLS"
    "Gateway/TLS/GatewayTLS|Gateway|TLS/GatewayTLS"
    "Gateway/ListenerHostname|Gateway|ListenerHostname"
    "Gateway/AllowedRoutes/Same|Gateway|AllowedRoutes/Same"
    "Gateway/AllowedRoutes/All|Gateway|AllowedRoutes/All"
    "Gateway/AllowedRoutes/Kinds|Gateway|AllowedRoutes/Kinds"
    "Gateway/AllowedRoutes/Selector|Gateway|AllowedRoutes/Selector"
    "Gateway/Combined|Gateway|Combined"
    "Gateway/StreamPlugins|Gateway|StreamPlugins"
    "HTTPRoute/Backend/BackendTLSSAN|Gateway|TLS/BackendTLSSAN"
    "Gateway/TLS/NoHostnameListener|Gateway|TLS/NoHostnameListener"
    "Gateway/FrontendValidation|Gateway|FrontendValidation"
    "Gateway/PortConflict|Gateway|PortConflict"
    "EdgionTls/https|EdgionTls|https"
    "EdgionTls/grpctls|EdgionTls|grpctls"
    "EdgionTls/mTLS|EdgionTls|mTLS"
    "EdgionTls/cipher|EdgionTls|cipher"
    "EdgionTls/mtls_failclose|EdgionTls|mtls_failclose"
    "EdgionTls/fallback_sni|EdgionTls|fallback_sni"
    "EdgionTls/BothAbsentParentRef|EdgionTls|BothAbsentParentRef"
    "EdgionPlugins/DebugAccessLog|EdgionPlugins|DebugAccessLog"
    "EdgionPlugins/Csrf|EdgionPlugins|Csrf"
    "EdgionPlugins/Cors|EdgionPlugins|Cors"
    "EdgionPlugins/PluginCondition|EdgionPlugins|PluginCondition"
    "EdgionPlugins/PluginCondition/AllConditions|EdgionPlugins|PluginCondition/AllConditions"
    "EdgionPlugins/CtxSet|EdgionPlugins|CtxSet"
    "EdgionPlugins/JwtAuth|EdgionPlugins|JwtAuth"
    "EdgionPlugins/JweDecrypt|EdgionPlugins|JweDecrypt"
    "EdgionPlugins/HmacAuth|EdgionPlugins|HmacAuth"
    "EdgionPlugins/HeaderCertAuth|EdgionPlugins|HeaderCertAuth"
    "EdgionPlugins/KeyAuth|EdgionPlugins|KeyAuth"
    "EdgionPlugins/BasicAuth|EdgionPlugins|BasicAuth"
    "EdgionPlugins/LdapAuth|EdgionPlugins|LdapAuth"
    "EdgionPlugins/ProxyRewrite|EdgionPlugins|ProxyRewrite"
    "EdgionPlugins/RateLimit|EdgionPlugins|RateLimit"
    "EdgionPlugins/RateLimitRedis|EdgionPlugins|RateLimitRedis"
    "EdgionPlugins/RealIp|EdgionPlugins|RealIp"
    "EdgionPlugins/RequestMirror|EdgionPlugins|RequestMirror"
    "EdgionPlugins/ResponseRewrite|EdgionPlugins|ResponseRewrite"
    "EdgionPlugins/RequestRestriction|EdgionPlugins|RequestRestriction"
    "EdgionPlugins/IpRestriction|EdgionPlugins|IpRestriction"
    "EdgionPlugins/PathNormalization|EdgionPlugins|PathNormalization"
    "EdgionPlugins/PathNormalization/ForwardView|EdgionPlugins|PathNormalization/ForwardView"
    "EdgionPlugins/FaultInjection|EdgionPlugins|FaultInjection"
    "EdgionPlugins/ForwardAuth|EdgionPlugins|ForwardAuth"
    "EdgionPlugins/ExtProc|EdgionPlugins|ExtProc"
    "EdgionPlugins/OpenidConnect|EdgionPlugins|OpenidConnect"
    "EdgionPlugins/BandwidthLimit|EdgionPlugins|BandwidthLimit"
    "EdgionPlugins/DirectEndpoint|EdgionPlugins|DirectEndpoint"
    "EdgionPlugins/DynamicInternalUpstream|EdgionPlugins|DynamicInternalUpstream"
    "EdgionPlugins/DynamicExternalUpstream|EdgionPlugins|DynamicExternalUpstream"
    "EdgionPlugins/RegionRoute|EdgionPlugins|RegionRoute"
    "EdgionPlugins/GlobalConnectionIpRestriction|EdgionPlugins|GlobalConnectionIpRestriction"
    "EdgionPlugins/EdgionConfigDataStatus|EdgionPlugins|EdgionConfigDataStatus"
    "EdgionPlugins/ResolvedRefs|EdgionPlugins|ResolvedRefs"
    "EdgionPlugins/WebhookKeyGet|EdgionPlugins|WebhookKeyGet"
    "EdgionPlugins/Dsl|EdgionPlugins|Dsl"
    "EdgionPlugins/Dsl|EdgionPlugins|DslGrpcError"
    "EdgionPlugins/AllEndpointStatus|EdgionPlugins|AllEndpointStatus"
    "Status/RefGrant|Status|RefGrant"
    "Status/RefGrant/TCPRouteCrossNsSvc|Status|RefGrant/TCPRouteCrossNsSvc"
    "Status/RefGrant/TLSRouteCrossNsSvc|Status|RefGrant/TLSRouteCrossNsSvc"
    "Status/BackendTLSPolicy|Status|BackendTLSPolicy"
    "Status/GatewayFrontendValidation|Status|GatewayFrontendValidation"
    "Status/TLSListenerMode|Status|TLSListenerMode"
    "Status/TLSRoute|Status|TLSRoute"
    "LinkSys/Status|LinkSys|Status"
  )

  local local_start_from="${START_FROM}"
  if [[ "${use_start_from}" != "true" ]]; then
    local_start_from=""
  fi

  # Phase 1: Classify suites into batch (in-pod) and host-side lists.
  local batch_suites=()    # resource|item pairs to run inside test-client pod
  local host_suites=()     # suite_dir|resource|item triples to run on host
  local batch_suite_keys=() # resource/item for display

  for pair in "${suites[@]}"; do
    local suite_dir rest resource item suite_key tname
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

    if ! suite_matches_filter "${resource}" "${item}"; then
      echo "Skip (filtered): ${suite_dir}"
      continue
    fi

    if [[ ! -d "${CONF_ROOT}/${suite_dir}" ]]; then
      echo "Skip suite: ${suite_dir} (missing config dir)"
      MISSING_COUNT=$((MISSING_COUNT + 1))
      MISSING_SUITES+=("${suite_dir}")
      continue
    fi

    tname="$(suite_test_name "${resource}" "${item}")"
    if [[ "${FULL_TEST}" != "true" ]] && is_slow_test "${tname}"; then
      echo "Skip slow suite (use --full-test): ${suite_dir}"
      continue
    fi

    SELECTED_COUNT=$((SELECTED_COUNT + 1))

    if is_host_side_suite "${resource}" "${item}"; then
      host_suites+=("${pair}")
    else
      batch_suites+=("${resource}|${item}")
      batch_suite_keys+=("${suite_key}")
    fi
  done

  # Phase 2: Run batch suites in a single kubectl exec.
  if [[ ${#batch_suites[@]} -gt 0 ]]; then
    local batch_tmp
    batch_tmp="$(mktemp /tmp/edgion-batch-suites.XXXXXX)"
    printf '%s\n' "${batch_suites[@]}" > "${batch_tmp}"

    echo
    echo "=============================="
    echo "Batch exec: ${#batch_suites[@]} suites in one kubectl session"
    echo "=============================="

    local batch_output batch_rc
    batch_output="$("${RUN_CLIENT_BATCH_SCRIPT}" "${batch_tmp}" 2>&1)" && batch_rc=0 || batch_rc=$?
    echo "${batch_output}"
    rm -f "${batch_tmp}"

    # Parse results from tagged output lines.
    local failed_in_batch=()
    while IFS= read -r line; do
      local result_key result_status
      result_key="$(echo "${line}" | awk '{print $2}')"
      result_status="$(echo "${line}" | awk '{print $3}')"
      local rsc itm
      rsc="${result_key%%|*}"
      itm="${result_key#*|}"
      local skey="${rsc}/${itm}"

      EXECUTED_COUNT=$((EXECUTED_COUNT + 1))
      if [[ "${result_status}" == "FAIL" ]]; then
        failed_in_batch+=("${result_key}")
      fi
    done < <(echo "${batch_output}" | grep '^@@SUITE_RESULT ')

    # Phase 2b: Retry failed suites individually (with run_one's retry logic).
    for fkey in ${failed_in_batch[@]+"${failed_in_batch[@]}"}; do
      local frsc fitm
      frsc="${fkey%%|*}"
      fitm="${fkey#*|}"
      local fdir=""
      for pair in "${suites[@]}"; do
        local sd sr si
        sd="${pair%%|*}"
        sr="${pair#*|}"
        sr="${sr%%|*}"
        si="${pair##*|}"
        if [[ "${sr}" == "${frsc}" && "${si}" == "${fitm}" ]]; then
          fdir="${sd}"
          break
        fi
      done

      echo
      echo "Retrying failed suite: ${frsc}/${fitm}"
      if ! run_one "${fdir}" "${frsc}" "${fitm}" "true"; then
        FAIL_COUNT=$((FAIL_COUNT + 1))
        FAILED_SUITES+=("${frsc}/${fitm}")
      fi
    done
  fi

  # Phase 3: Run host-side suites individually.
  for pair in ${host_suites[@]+"${host_suites[@]}"}; do
    local suite_dir rest resource item
    suite_dir="${pair%%|*}"
    rest="${pair#*|}"
    resource="${rest%%|*}"
    item="${rest#*|}"

    if ! run_one "${suite_dir}" "${resource}" "${item}"; then
      FAIL_COUNT=$((FAIL_COUNT + 1))
      FAILED_SUITES+=("${resource}/${item}")
    fi
  done
}

should_run_prepare() {
  if [[ "${SKIP_PREPARE}" == "true" ]]; then
    return 1
  fi
  if [[ "${FORCE_PREPARE}" == "true" ]]; then
    return 0
  fi
  # Auto-detect: if state ConfigMap exists and matches, skip prepare.
  local conf_hash
  conf_hash="$(compute_prepare_conf_hash)"
  if ! kubectl -n "${STATE_NAMESPACE}" get configmap "${STATE_CONFIGMAP_NAME}" >/dev/null 2>&1; then
    return 0
  fi
  local stored_ok stored_hash stored_replicas stored_ns
  stored_ok="$(get_state_data prepare_ok)"
  stored_hash="$(get_state_data conf_hash)"
  stored_replicas="$(get_state_data test_server_replicas)"
  stored_ns="$(get_state_data backend_test_namespace)"
  if [[ "${stored_ok}" == "true" \
     && "${stored_hash}" == "${conf_hash}" \
     && "${stored_replicas}" == "${TEST_SERVER_REPLICAS}" \
     && "${stored_ns}" == "${BACKEND_TEST_NAMESPACE}" ]]; then
    return 1
  fi
  return 0
}

if should_run_prepare; then
  echo "Phase 1/2: Prepare environment"
  echo "Using BACKEND_TEST_NAMESPACE=${BACKEND_TEST_NAMESPACE}"
  print_cluster_and_image_context
  PREPARE_CONF_HASH="$(compute_prepare_conf_hash)"
  "${VALIDATE_SCRIPT}" "${CONF_ROOT}"

  if [[ "${SKIP_DEPLOY}" == "false" ]]; then
    # Ensure namespaces exist before applying generated secrets.
    if [[ -f "${K8S_DEPLOY_ROOT}/namespace.yaml" ]]; then
      kubectl apply --server-side --force-conflicts -f "${K8S_DEPLOY_ROOT}/namespace.yaml"
    fi
  fi
  if [[ -f "${CONF_ROOT}/Status/00_Namespace_status-compat.yaml" ]]; then
    kubectl apply --server-side --force-conflicts -f "${CONF_ROOT}/Status/00_Namespace_status-compat.yaml"
  fi

  echo "Generate runtime TLS/mTLS secrets..."
  "${GENERATE_CERTS_SCRIPT}" "${GENERATED_DIR}"
  if [[ ! -d "${GENERATED_SECRET_DIR}" ]]; then
    echo "generated secret dir not found: ${GENERATED_SECRET_DIR}"
    exit 1
  fi
  kubectl apply --server-side --force-conflicts --field-manager=edgion-k8s-test -f "${GENERATED_SECRET_DIR}"

  # The gateway Deployment mounts EDGION_GATEWAY_ADMIN_TOKEN from this Secret via
  # secretKeyRef; without it the gateway Pod is stuck in CreateContainerConfigError.
  # Create it (idempotent apply) before the Deployment is applied below.
  echo "Ensure gateway admin token Secret (edgion-gateway-admin-credentials)..."
  kubectl -n edgion-system create secret generic edgion-gateway-admin-credentials \
    --from-literal=token="${GATEWAY_ADMIN_TOKEN}" \
    --dry-run=client -o yaml | kubectl apply -f -

  if [[ "${SKIP_DEPLOY}" == "false" ]]; then
    export CONTROLLER_IMAGE GATEWAY_IMAGE TEST_CLIENT_IMAGE TEST_SERVER_IMAGE PULL_POLICY_OVERRIDE
    deploy_args=(--test-server-replicas "${TEST_SERVER_REPLICAS}")
    if local_test_binary_mode; then
      deploy_args+=(--no-wait)
    fi
    K8S_DEPLOY_ROOT="$K8S_DEPLOY_ROOT" BACKEND_TEST_NAMESPACE="${BACKEND_TEST_NAMESPACE}" "${DEPLOY_SCRIPT}" \
      "${deploy_args[@]}"
    if local_test_binary_mode; then
      echo "Waiting controller rollout..."
      kubectl rollout status deployment/edgion-controller -n edgion-system --timeout=300s
      echo "Waiting gateway rollout..."
      kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=300s
    fi
    install_local_core_binaries
  fi

  ensure_gatewayclass_compat
  apply_all_conf_with_fallback
  sync_k8s_redis_auth_secret
  if local_test_binary_mode; then
    patch_local_binary_test_deployments
  else
    override_suite_test_server_images
  fi
  restart_secret_dependent_workloads
  if local_test_binary_mode; then
    echo "Restarting test-client after ConfigMap updates..."
    kubectl rollout restart deployment/edgion-test-client -n edgion-test
    kubectl rollout status deployment/edgion-test-client -n edgion-test --timeout=300s
  fi
  install_local_test_binaries

  CLUSTER_PATCH_SCRIPT="${SCRIPT_DIR}/cluster-patch.sh"
  if [[ -x "${CLUSTER_PATCH_SCRIPT}" ]]; then
    echo "Running cluster-specific patches..."
    BACKEND_TEST_NAMESPACE="${BACKEND_TEST_NAMESPACE}" \
      K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT}" \
      bash "${CLUSTER_PATCH_SCRIPT}"
  fi

  echo "Restarting gateway after full apply..."
  kubectl rollout restart deployment/edgion-gateway -n edgion-system
  kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=300s
  install_local_core_binaries
  wait_gateway_stable 300

  force_sync_secrets

  write_prepare_state_marker "${PREPARE_CONF_HASH}"
  echo "Prepare phase finished."
else
  if [[ "${SKIP_PREPARE}" == "true" ]]; then
    echo "Phase 1/2: Skipped (--skip-prepare)"
  else
    echo "Phase 1/2: Skipped (state ConfigMap up-to-date, use --force-prepare to override)"
  fi
  print_cluster_and_image_context
  verify_skip_prepare_preconditions
  install_local_core_binaries
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
  g_before="$(get_gateway_server_id)"
  echo "Before reload: controller=${c_before} gateway=${g_before}"

  echo "Test round #1"
  run_all_selected_suites "true"

  echo "Triggering reload..."
  trigger_reload "${CONTROLLER_ADMIN_URL}"
  sleep 3
  wait_gateway_stable 120

  c_after="$(get_server_id "${CONTROLLER_ADMIN_URL}")"
  g_after="$(get_gateway_server_id)"
  echo "After reload: controller=${c_after} gateway=${g_after}"

  # An empty or "unknown" server_id means we never reached the admin API, so the
  # comparison is meaningless. Two "unknown" values would otherwise compare equal on
  # the controller side or unequal-by-accident on the gateway side and silently
  # mislead, so treat any unresolved id as a hard error.
  for pair in "controller:${c_before}" "controller:${c_after}" "gateway:${g_before}" "gateway:${g_after}"; do
    if [[ -z "${pair#*:}" || "${pair#*:}" == "unknown" ]]; then
      echo "reload check failed: ${pair%%:*} server_id unresolved (empty/unknown); cannot verify reload"
      exit 1
    fi
  done

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

PASS_COUNT=$((EXECUTED_COUNT - FAIL_COUNT))
if [[ "${EXECUTED_COUNT}" -gt 0 ]]; then
  PASS_RATE=$((PASS_COUNT * 100 / EXECUTED_COUNT))
else
  PASS_RATE=0
fi

echo
echo "=========================================================="
echo "  K8s Integration Test Summary"
echo "=========================================================="
echo "  Selected : ${SELECTED_COUNT} suites"
echo "  Executed : ${EXECUTED_COUNT} suites"
echo "  Passed   : ${PASS_COUNT}"
echo "  Failed   : ${FAIL_COUNT}"
echo "  Skipped  : $((SELECTED_COUNT - EXECUTED_COUNT))"
if [[ "${MISSING_COUNT}" -gt 0 ]]; then
  echo "  Missing  : ${MISSING_COUNT} (no conf dir)"
fi
echo "  Pass rate: ${PASS_RATE}%"
MODE_STR="fast"
if [[ "${FULL_TEST}" == "true" ]]; then MODE_STR="full"; fi
if [[ "${WITH_RELOAD}" == "true" ]]; then MODE_STR="${MODE_STR} + reload"; fi
echo "  Mode     : ${MODE_STR}"
echo "=========================================================="
if [[ "${FAIL_COUNT}" -gt 0 ]]; then
  echo
  echo "  Failed suites:"
  for s in ${FAILED_SUITES[@]+"${FAILED_SUITES[@]}"}; do
    echo "    ✗ ${s}"
  done
  echo
  echo "=========================================================="
fi
echo "  Cluster state is kept. Run cleanup.sh to reset."
echo "=========================================================="

if [[ "${FAIL_COUNT}" -gt 0 ]]; then
  exit 1
fi
