#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/conf}"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/kubernetes}"
COMMON_SCRIPT_DIR="${SCRIPT_DIR}/common"
ORBSTACK_SCRIPT_DIR="${SCRIPT_DIR}/orbstack"

VALIDATE_SCRIPT="$COMMON_SCRIPT_DIR/validate_no_endpoints.sh"
FIXTURE_OWNERSHIP_VALIDATOR="$COMMON_SCRIPT_DIR/validate_fixture_ownership.sh"
DEPLOY_SCRIPT="$COMMON_SCRIPT_DIR/deploy_integration.sh"
APPLY_ALL_SCRIPT="$COMMON_SCRIPT_DIR/apply_all_conf_strict.sh"
RUN_CLIENT_SCRIPT="$COMMON_SCRIPT_DIR/run_test_client.sh"
RUN_CLIENT_BATCH_SCRIPT="$COMMON_SCRIPT_DIR/run_test_client_batch.sh"
BATCH_RESULTS_VALIDATOR="$COMMON_SCRIPT_DIR/validate_batch_results.sh"
ENDPOINT_INVENTORY="$COMMON_SCRIPT_DIR/endpoint_inventory.sh"
TEST_CLIENT_PEER_IDENTITY="$COMMON_SCRIPT_DIR/test_client_peer_identity.sh"
RUN_DYNAMIC_SCRIPT="$COMMON_SCRIPT_DIR/run_k8s_dynamic_scenarios.sh"
K8S_DYNAMIC_MANIFEST="$COMMON_SCRIPT_DIR/k8s_dynamic_scenarios.tsv"
RUN_ISOLATED_JOBS_SCRIPT="$COMMON_SCRIPT_DIR/run_k8s_isolated_jobs.sh"
RUN_PROXY_PROTOCOL_JOBS_SCRIPT="$COMMON_SCRIPT_DIR/run_k8s_proxy_protocol_jobs.sh"
RUN_METRICS_LABEL_GATE="$COMMON_SCRIPT_DIR/run_metrics_label_gate.sh"
RUN_UNKNOWN_CONFIG_FIELDS_GATE="$COMMON_SCRIPT_DIR/run_unknown_config_fields_gate.sh"
RUN_REQUEUE_DEAD_LOOP_GATE="$COMMON_SCRIPT_DIR/run_requeue_dead_loop_gate.sh"
RUN_CACHE_RESIDUE_GATE="$COMMON_SCRIPT_DIR/run_cache_residue_gate.sh"
CLEANUP_SCRIPT="$SCRIPT_DIR/cleanup.sh"
GENERATE_CERTS_SCRIPT="$COMMON_SCRIPT_DIR/generate_runtime_certs.sh"
CREDENTIAL_SOURCE_OAUTH2_SCRIPT="$COMMON_SCRIPT_DIR/test_credential_source_oauth2.sh"
CREDENTIAL_SOURCE_OAUTH2_MEMORY_SCRIPT="$COMMON_SCRIPT_DIR/test_credential_source_oauth2_memory.sh"
LOCAL_IMAGE_PREPARE_SCRIPT="$ORBSTACK_SCRIPT_DIR/prepare_local_images.sh"
GENERATED_DIR="${GENERATED_DIR:-$PROJECT_ROOT/edgion-tests/k8s/generated}"
GENERATED_SECRET_DIR="${GENERATED_DIR}/secrets"
PORTS_CONFIG="${PORTS_CONFIG:-$PROJECT_ROOT/edgion-tests/integration/conf/ports.json}"
IMAGE_CONFIG_FILE="${IMAGE_CONFIG_FILE:-$SCRIPT_DIR/conf.env}"
STATE_NAMESPACE="${STATE_NAMESPACE:-edgion-system}"
STATE_CONFIGMAP_NAME="${STATE_CONFIGMAP_NAME:-edgion-k8s-integration-state}"
PREPARE_STATE_OWNER="edgion-k8s-integration-v1"
GATEWAY_SYNC_READY_TIMEOUT_SECONDS="${EDGION_K8S_GATEWAY_SYNC_READY_TIMEOUT_SECONDS:-300}"

# Create a durable run record before any cluster mutation. A process that exits
# before the final summary leaves INCOMPLETE, never an accidental pass.
K8S_REPORT_ROOT="${K8S_REPORT_ROOT:-${PROJECT_ROOT}/tmp/k8s_integration}"
K8S_RUN_ID="${K8S_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
K8S_RUN_DIR="${K8S_REPORT_ROOT}/${K8S_RUN_ID}"
K8S_RUN_STATUS_FILE="${K8S_RUN_DIR}/status"
K8S_RUN_LOG="${K8S_RUN_DIR}/run.log"
RUN_OWNERSHIP_FILE="${K8S_RUN_DIR}/owned-cluster-keys.tsv"
K8S_RUN_FINALIZED=false
CURRENT_ROUND="round-1"
RUN_OWNS_CLUSTER=false
REPREPARE_EXISTING=false
_PF_PIDS=()

collect_k8s_diagnostics() {
  command -v kubectl >/dev/null 2>&1 || return 0
  kubectl --request-timeout=10s get pods -A -o wide > "${K8S_RUN_DIR}/pods.txt" 2>&1 || true
  kubectl --request-timeout=10s get events -A --sort-by=.lastTimestamp \
    > "${K8S_RUN_DIR}/events.txt" 2>&1 || true
  kubectl --request-timeout=10s get endpointslice -A -o yaml \
    > "${K8S_RUN_DIR}/endpointslices.yaml" 2>&1 || true
  kubectl --request-timeout=10s get gateway,httproute,grpcroute,tcproute,tlsroute,udproute -A -o yaml \
    > "${K8S_RUN_DIR}/route-status.yaml" 2>&1 || true
  kubectl --request-timeout=10s get deployment,statefulset,daemonset -A -o wide \
    > "${K8S_RUN_DIR}/workloads.txt" 2>&1 || true
  kubectl --request-timeout=10s logs -n edgion-system -l app=edgion-controller \
    --all-containers=true --prefix=true --since-time="${RUN_START_TIME:-1970-01-01T00:00:00Z}" \
    > "${K8S_RUN_DIR}/controller.log" 2>&1 || true
  kubectl --request-timeout=10s logs -n edgion-system -l app=edgion-gateway \
    --all-containers=true --prefix=true --since-time="${RUN_START_TIME:-1970-01-01T00:00:00Z}" \
    > "${K8S_RUN_DIR}/gateway.log" 2>&1 || true
  {
    while IFS=$'\t' read -r namespace pod; do
      [[ -n "${namespace}" && -n "${pod}" ]] || continue
      echo "Pod: ${namespace}/${pod}"
      kubectl --request-timeout=10s describe pod -n "${namespace}" "${pod}" || true
      echo "Current logs: ${namespace}/${pod}"
      kubectl --request-timeout=10s logs -n "${namespace}" "${pod}" \
        --all-containers=true --prefix=true --since-time="${RUN_START_TIME:-1970-01-01T00:00:00Z}" || true
      echo "Previous logs: ${namespace}/${pod}"
      kubectl --request-timeout=10s logs -n "${namespace}" "${pod}" \
        --all-containers=true --prefix=true --previous || true
    done < <(
      kubectl --request-timeout=10s get pods -A -o json 2>/dev/null \
        | jq -r '.items[]
          | select(any(.status.conditions[]?; .type == "Ready" and .status != "True"))
          | [.metadata.namespace, .metadata.name]
          | @tsv'
    )
  } > "${K8S_RUN_DIR}/non-ready-pods.log" 2>&1 || true
}

write_run_ownership_inventory() {
  python3 - "${CONF_ROOT}" "${K8S_DEPLOY_ROOT}" <<'PY'
import pathlib
import sys

import yaml

cluster_kinds = {
    "ClusterRole",
    "ClusterRoleBinding",
    "EdgionGatewayConfig",
    "GatewayClass",
}
keys = set()
for raw_root in sys.argv[1:]:
    root = pathlib.Path(raw_root)
    if not root.exists():
        continue
    for path in sorted(set(root.rglob("*.yaml")) | set(root.rglob("*.yml"))):
        if path.name.endswith(".template.yaml"):
            continue
        try:
            documents = list(yaml.safe_load_all(path.read_text()))
        except yaml.YAMLError as error:
            raise SystemExit(f"invalid ownership-inventory YAML {path}: {error}") from error
        for document in documents:
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

assert_clean_run_boundary() {
  local scope kind locator name output conflicts=0
  write_run_ownership_inventory > "${RUN_OWNERSHIP_FILE}"
  while IFS=$'\t' read -r scope kind locator name; do
    [[ -n "${name}" ]] || continue
    if [[ "${scope}" == "namespace" ]]; then
      if ! output="$(kubectl get namespace "${name}" --ignore-not-found -o name 2>&1)"; then
        echo "Run ownership probe failed for namespace/${name}: ${output}" >&2
        return 1
      elif [[ -n "${output}" ]]; then
        echo "Run ownership conflict: namespace/${name} already exists" >&2
        conflicts=$((conflicts + 1))
      fi
    elif ! output="$(kubectl get "${locator}" "${name}" --ignore-not-found -o name 2>&1)"; then
      echo "Run ownership probe failed for ${kind}/${name}: ${output}" >&2
      return 1
    elif [[ -n "${output}" ]]; then
      echo "Run ownership conflict: ${kind}/${name} already exists" >&2
      conflicts=$((conflicts + 1))
    fi
  done < "${RUN_OWNERSHIP_FILE}"

  if [[ "${conflicts}" -ne 0 ]]; then
    echo "Default prepare requires a clean integration boundary. Run cleanup.sh first or use --keep-alive for an explicitly preserved developer cluster." >&2
    return 1
  fi
  RUN_OWNS_CLUSTER=true
}

verify_run_cleanup() {
  local scope kind locator name output rc residue=0
  [[ -f "${RUN_OWNERSHIP_FILE}" ]] || return 0
  while IFS=$'\t' read -r scope kind locator name; do
    [[ -n "${name}" ]] || continue
    if [[ "${scope}" == "namespace" ]]; then
      output="$(kubectl get namespace "${name}" --ignore-not-found -o name 2>&1)" && rc=0 || rc=$?
      if [[ "${rc}" -ne 0 ]]; then
        echo "Cleanup verification failed for namespace/${name}: ${output}" >&2
        residue=$((residue + 1))
      elif [[ -n "${output}" ]]; then
        echo "Cleanup residue: namespace/${name}" >&2
        residue=$((residue + 1))
      fi
    else
      output="$(kubectl get "${locator}" "${name}" --ignore-not-found -o name 2>&1)" && rc=0 || rc=$?
      if [[ "${rc}" -ne 0 ]]; then
        echo "Cleanup verification failed for ${kind}/${name}: ${output}" >&2
        residue=$((residue + 1))
      elif [[ -n "${output}" ]]; then
        echo "Cleanup residue: ${kind}/${name}" >&2
        residue=$((residue + 1))
      fi
    fi
  done < "${RUN_OWNERSHIP_FILE}"
  [[ "${residue}" -eq 0 ]]
}

finish_k8s_run() {
  local exit_code=$?
  set +e
  local p
  for p in "${_PF_PIDS[@]:-}"; do
    kill "${p}" 2>/dev/null || true
    wait "${p}" 2>/dev/null || true
  done
  collect_k8s_diagnostics
  if [[ "${RUN_OWNS_CLUSTER}" == "true" && "${KEEP_ALIVE:-false}" != "true" ]]; then
    echo "Cleaning resources owned by K8s integration run ${K8S_RUN_ID}..."
    if ! "${CLEANUP_SCRIPT}" --ownership-file "${RUN_OWNERSHIP_FILE}" > "${K8S_RUN_DIR}/cleanup.log" 2>&1; then
      echo "Run-owned cleanup failed; see ${K8S_RUN_DIR}/cleanup.log"
      exit_code=1
    elif ! verify_run_cleanup > "${K8S_RUN_DIR}/cleanup-verify.log" 2>&1; then
      echo "Run-owned cleanup left residue; see ${K8S_RUN_DIR}/cleanup-verify.log"
      exit_code=1
    fi
  fi
  if [[ "${K8S_RUN_FINALIZED}" != "true" ]]; then
    printf 'INCOMPLETE\n' > "${K8S_RUN_STATUS_FILE}"
  elif [[ "${exit_code}" -eq 0 ]]; then
    printf 'PASS\n' > "${K8S_RUN_STATUS_FILE}"
  else
    printf 'FAIL\n' > "${K8S_RUN_STATUS_FILE}"
  fi
  printf 'finished_at=%s\nexit_code=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${exit_code}" \
    >> "${K8S_RUN_DIR}/metadata"
  echo "K8s run artifacts: ${K8S_RUN_DIR}"
  return "${exit_code}"
}

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
KEEP_ALIVE=false
TEST_SERVER_REPLICAS="${TEST_SERVER_REPLICAS:-3}"
SPEC_PROFILE="${SPEC_PROFILE:-}"
FULL_TEST=false
WITH_RELOAD=false
BACKEND_TEST_NAMESPACE="${BACKEND_TEST_NAMESPACE:-edgion-backend}"
CLUSTER_MODE="${CLUSTER_MODE:-auto}" # auto|local|remote
LOCAL_IMAGE_MODE="${LOCAL_IMAGE_MODE:-auto}" # auto|remote-pull|local-existing|build-import|prepare-local-images
TEST_CLIENT_IMAGE="${TEST_CLIENT_IMAGE:-}"
TEST_SERVER_IMAGE="${TEST_SERVER_IMAGE:-}"
CONTROLLER_IMAGE="${CONTROLLER_IMAGE:-}"
GATEWAY_IMAGE="${GATEWAY_IMAGE:-}"
USER_TEST_CLIENT_IMAGE="${TEST_CLIENT_IMAGE}"
USER_TEST_SERVER_IMAGE="${TEST_SERVER_IMAGE}"
USER_CONTROLLER_IMAGE="${CONTROLLER_IMAGE}"
USER_GATEWAY_IMAGE="${GATEWAY_IMAGE}"
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
ISOLATED_JOB_IMAGE="${EDGION_ISOLATED_JOB_IMAGE:-}"
USER_ISOLATED_JOB_IMAGE="${ISOLATED_JOB_IMAGE}"
HAPROXY_IMAGE="${EDGION_HAPROXY_IMAGE:-haproxy:3.0-alpine}"
ISOLATED_IMAGE_PULL_POLICY="${EDGION_ISOLATED_IMAGE_PULL_POLICY:-IfNotPresent}"
HAPROXY_IMAGE_PULL_POLICY="${EDGION_HAPROXY_IMAGE_PULL_POLICY:-IfNotPresent}"
ISOLATED_DEADLINE_GRACE_SECONDS="${EDGION_ISOLATED_DEADLINE_GRACE_SECONDS:-30}"
PROXY_TOPOLOGY_TIMEOUT_SECONDS="${EDGION_PROXY_TOPOLOGY_TIMEOUT_SECONDS:-420}"

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
HARNESS_FAILED=false
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
  --local-image-mode <auto|remote-pull|local-existing|build-import|prepare-local-images>
                                 Local cluster image mode (default: ${LOCAL_IMAGE_MODE})
  --image-config <path>          Image config file (default: scripts/conf.env)
  --prepare-local-images         Build local test images and auto-set TEST_*_IMAGE
  --rebuild-local-images         With --prepare-local-images, force rebuild
  --full-test                    Include slow tests (Timeout/AllEndpointStatus/LdapAuth)
  --with-reload                  Run test rounds twice with reload in between
  --keep-alive                   Preserve the integration cluster after the run
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
  EDGION_ISOLATED_JOB_IMAGE      Dedicated image containing all isolated-test binaries
                                 and assets; required for an unfiltered test run
  EDGION_HAPROXY_IMAGE           HAProxy image used by isolated PROXY protocol tests
                                 (default: ${HAPROXY_IMAGE})
  EDGION_K8S_EXPECTED_CONTEXT    Optional exact kubectl context guard; fail before
                                 mutation when the active context differs
  EDGION_ISOLATED_IMAGE_PULL_POLICY
                                 Pull policy for the isolated-test image
  EDGION_HAPROXY_IMAGE_PULL_POLICY
                                 Pull policy for the isolated HAProxy image
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
         && "${LOCAL_IMAGE_MODE}" != "local-existing" \
         && "${LOCAL_IMAGE_MODE}" != "build-import" && "${LOCAL_IMAGE_MODE}" != "prepare-local-images" ]]; then
        echo "invalid --local-image-mode: ${LOCAL_IMAGE_MODE} (expected: auto|remote-pull|local-existing|build-import|prepare-local-images)"
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
    --keep-alive)
      KEEP_ALIVE=true
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
if [[ "${PREPARE_ONLY}" == "true" ]]; then
  # The purpose of prepare-only is to leave a reusable prepared cluster.
  KEEP_ALIVE=true
fi
if ! [[ "${GATEWAY_SYNC_READY_TIMEOUT_SECONDS}" =~ ^[1-9][0-9]*$ ]]; then
  echo "invalid EDGION_K8S_GATEWAY_SYNC_READY_TIMEOUT_SECONDS: ${GATEWAY_SYNC_READY_TIMEOUT_SECONDS}"
  exit 1
fi

for required in "$VALIDATE_SCRIPT" "$FIXTURE_OWNERSHIP_VALIDATOR" "$DEPLOY_SCRIPT" "$APPLY_ALL_SCRIPT" "$RUN_CLIENT_SCRIPT" "$RUN_CLIENT_BATCH_SCRIPT" "$BATCH_RESULTS_VALIDATOR" "$ENDPOINT_INVENTORY" "$TEST_CLIENT_PEER_IDENTITY" "$RUN_DYNAMIC_SCRIPT" "$RUN_ISOLATED_JOBS_SCRIPT" "$RUN_PROXY_PROTOCOL_JOBS_SCRIPT" "$RUN_METRICS_LABEL_GATE" "$RUN_UNKNOWN_CONFIG_FIELDS_GATE" "$RUN_REQUEUE_DEAD_LOOP_GATE" "$RUN_CACHE_RESIDUE_GATE" "$GENERATE_CERTS_SCRIPT" "$CREDENTIAL_SOURCE_OAUTH2_SCRIPT" "$CREDENTIAL_SOURCE_OAUTH2_MEMORY_SCRIPT" "$CLEANUP_SCRIPT"; do
  if [[ ! -x "$required" ]]; then
    echo "required script not found or not executable: $required"
    exit 1
  fi
done

case "${ISOLATED_IMAGE_PULL_POLICY}" in
  Always|IfNotPresent|Never) ;;
  *) echo "invalid EDGION_ISOLATED_IMAGE_PULL_POLICY: ${ISOLATED_IMAGE_PULL_POLICY}"; exit 1 ;;
esac
case "${HAPROXY_IMAGE_PULL_POLICY}" in
  Always|IfNotPresent|Never) ;;
  *) echo "invalid EDGION_HAPROXY_IMAGE_PULL_POLICY: ${HAPROXY_IMAGE_PULL_POLICY}"; exit 1 ;;
esac
if [[ ! -f "${K8S_DYNAMIC_MANIFEST}" ]]; then
  echo "required K8s dynamic manifest not found: ${K8S_DYNAMIC_MANIFEST}"
  exit 1
fi

# shellcheck source=common/validate_batch_results.sh
source "${BATCH_RESULTS_VALIDATOR}"
# shellcheck source=common/endpoint_inventory.sh
source "${ENDPOINT_INVENTORY}"

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
if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 not found"
  exit 1
fi
if ! python3 -c 'import yaml' >/dev/null 2>&1; then
  echo "python3 package PyYAML is required by the strict K8s fixture loader"
  exit 1
fi

# Establish the durable run boundary after side-effect-free argument/tool
# preflight, but before the first cluster query or mutation. Help and malformed
# invocations therefore remain side-effect free, while every real run starts as
# INCOMPLETE until it reaches an explicit terminal summary.
mkdir -p "${K8S_RUN_DIR}"
printf 'INCOMPLETE\n' > "${K8S_RUN_STATUS_FILE}"
printf 'run_id=%s\nstarted_at=%s\n' "${K8S_RUN_ID}" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  > "${K8S_RUN_DIR}/metadata"
exec > >(tee -a "${K8S_RUN_LOG}") 2>&1
trap finish_k8s_run EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Capture one run boundary before any deployment, fixture apply, restart, or
# other cluster mutation. The RequeueDeadLoop gate uses this exact timestamp
# for every current/previous Controller log request.
RUN_START_TIME="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

preflight_kubernetes_cluster() {
  local context version_output nodes_json node_count ready_count non_ready_count architectures

  if ! context="$(kubectl config current-context 2>&1)" || [[ -z "${context}" ]]; then
    echo "Kubernetes preflight failed: kubectl has no current context" >&2
    return 1
  fi
  if [[ -n "${EDGION_K8S_EXPECTED_CONTEXT:-}" && "${context}" != "${EDGION_K8S_EXPECTED_CONTEXT}" ]]; then
    echo "Kubernetes preflight failed: current context ${context} does not match EDGION_K8S_EXPECTED_CONTEXT=${EDGION_K8S_EXPECTED_CONTEXT}" >&2
    return 1
  fi

  if ! version_output="$(kubectl --request-timeout=10s version --output=json 2>&1)"; then
    echo "Kubernetes preflight failed: API server is not reachable through context ${context}" >&2
    echo "${version_output}" >&2
    return 1
  fi

  if ! nodes_json="$(kubectl --request-timeout=10s get nodes -o json 2>&1)"; then
    echo "Kubernetes preflight failed: cannot list Nodes through context ${context}" >&2
    echo "${nodes_json}" >&2
    return 1
  fi
  if ! echo "${nodes_json}" | jq -e '.kind == "NodeList" and (.items | type == "array")' >/dev/null; then
    echo "Kubernetes preflight failed: kubectl returned an invalid NodeList" >&2
    return 1
  fi

  node_count="$(echo "${nodes_json}" | jq '.items | length')"
  ready_count="$(echo "${nodes_json}" | jq '[.items[] | select(any(.status.conditions[]?; .type == "Ready" and .status == "True"))] | length')"
  non_ready_count=$((node_count - ready_count))
  if [[ "${node_count}" -eq 0 ]]; then
    echo "Kubernetes preflight failed: context ${context} has no Nodes" >&2
    return 1
  fi
  if [[ "${ready_count}" -eq 0 ]]; then
    echo "Kubernetes preflight failed: context ${context} has no Ready Nodes" >&2
    return 1
  fi

  architectures="$(echo "${nodes_json}" | jq -r '.items[] | select(any(.status.conditions[]?; .type == "Ready" and .status == "True")) | .status.nodeInfo.architecture // empty' | sort -u | xargs)"
  if [[ -z "${architectures}" ]]; then
    echo "Kubernetes preflight failed: Ready Nodes do not report an architecture" >&2
    return 1
  fi

  echo "Kubernetes preflight passed: context=${context} ready_nodes=${ready_count}/${node_count} architectures=${architectures}"
  if [[ "${non_ready_count}" -gt 0 ]]; then
    echo "[WARN] Kubernetes context ${context} also has ${non_ready_count} non-Ready Node(s)"
  fi
}

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

  # Preserve auto-detected IMAGE_ARCH; let the config file fill other values.
  local _saved_arch="${IMAGE_ARCH:-}"

  # Clear values derived by an earlier config load. Only the original user
  # environment is an override; otherwise a prepare-local-images rewrite must
  # be allowed to replace the first config's effective image references.
  TEST_CLIENT_IMAGE="${USER_TEST_CLIENT_IMAGE}"
  TEST_SERVER_IMAGE="${USER_TEST_SERVER_IMAGE}"
  CONTROLLER_IMAGE="${USER_CONTROLLER_IMAGE}"
  GATEWAY_IMAGE="${USER_GATEWAY_IMAGE}"
  ISOLATED_JOB_IMAGE="${USER_ISOLATED_JOB_IMAGE}"

  # shellcheck disable=SC1090
  source "${f}"

  # Restore auto-detected arch when the config file would downgrade it.
  if [[ -n "${_saved_arch}" ]]; then
    IMAGE_ARCH="${_saved_arch}"
  fi
  if [[ -n "${USER_TEST_CLIENT_IMAGE}" ]]; then
    TEST_CLIENT_IMAGE="${USER_TEST_CLIENT_IMAGE}"
  fi
  if [[ -n "${USER_TEST_SERVER_IMAGE}" ]]; then
    TEST_SERVER_IMAGE="${USER_TEST_SERVER_IMAGE}"
  fi
  if [[ -n "${USER_CONTROLLER_IMAGE}" ]]; then
    CONTROLLER_IMAGE="${USER_CONTROLLER_IMAGE}"
  fi
  if [[ -n "${USER_GATEWAY_IMAGE}" ]]; then
    GATEWAY_IMAGE="${USER_GATEWAY_IMAGE}"
  fi
  if [[ -n "${USER_ISOLATED_JOB_IMAGE}" ]]; then
    ISOLATED_JOB_IMAGE="${USER_ISOLATED_JOB_IMAGE}"
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

require_local_image_build_tooling() {
  local mode="$1"
  if [[ "${mode}" != "build-import" && "${mode}" != "prepare-local-images" && "${PREPARE_LOCAL_IMAGES}" != "true" ]]; then
    return 0
  fi
  if ! command -v docker >/dev/null 2>&1; then
    echo "Local image mode ${mode} requires docker" >&2
    exit 1
  fi
  if ! docker info >/dev/null 2>&1; then
    echo "Local image mode ${mode} requires a reachable Docker daemon" >&2
    exit 1
  fi
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
  2) local-existing       use images already visible to the local runtime, use IfNotPresent
  3) build-import         build local images, docker save, import into local runtime, use IfNotPresent
  4) prepare-local-images run existing --prepare-local-images flow, use IfNotPresent
EOF
  read -r -p "Select [1/2/3/4] (default 1): " selected >&2
  case "${selected:-1}" in
    2) echo "local-existing" ;;
    3) echo "build-import" ;;
    4) echo "prepare-local-images" ;;
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
  local mode arches primary local_mode build_import_local_images=false
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
      local-existing)
        PULL_POLICY_OVERRIDE="IfNotPresent"
        ;;
      build-import)
        build_import_local_images=true
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

  require_local_image_build_tooling "${LOCAL_IMAGE_MODE_RESOLVED}"
  if [[ "${build_import_local_images}" == "true" ]]; then
    build_and_import_local_images
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
  echo "Image override EDGION_ISOLATED_JOB_IMAGE: ${ISOLATED_JOB_IMAGE:-<unset>}"
  echo "Image override EDGION_HAPROXY_IMAGE: ${HAPROXY_IMAGE}"
  echo "Pull policy override: ${PULL_POLICY_OVERRIDE:-<none>}"

  check_image_arch_compat "${CONTROLLER_IMAGE}" "${primary}" "CONTROLLER_IMAGE"
  check_image_arch_compat "${GATEWAY_IMAGE}" "${primary}" "GATEWAY_IMAGE"
  check_image_arch_compat "${TEST_CLIENT_IMAGE}" "${primary}" "TEST_CLIENT_IMAGE"
  check_image_arch_compat "${TEST_SERVER_IMAGE}" "${primary}" "TEST_SERVER_IMAGE"
  check_image_arch_compat "${ISOLATED_JOB_IMAGE}" "${primary}" "EDGION_ISOLATED_JOB_IMAGE"
  check_image_arch_compat "${HAPROXY_IMAGE}" "${primary}" "EDGION_HAPROXY_IMAGE"

  if [[ "${mode}" == "remote" && "${SKIP_DEPLOY}" == "false" ]]; then
    if [[ -z "${CONTROLLER_IMAGE}" || -z "${GATEWAY_IMAGE}" || -z "${TEST_CLIENT_IMAGE}" || -z "${TEST_SERVER_IMAGE}" ]]; then
      echo "[WARN] remote mode detected but some images are not set."
      echo "[WARN] IMAGE_ARCH=${IMAGE_ARCH} should auto-resolve them; check images-remote.env or set explicitly."
    fi
  fi
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
  local file_list hash_input f digest identity result
  file_list="$(mktemp /tmp/edgion-k8s-prepare-files.XXXXXX)" || return 1
  hash_input="$(mktemp /tmp/edgion-k8s-prepare-input.XXXXXX)" || {
    rm -f "${file_list}"
    return 1
  }

  for f in "${CONF_ROOT}" "${K8S_DEPLOY_ROOT}"; do
    if [[ ! -d "${f}" ]]; then
      echo "prepare hash input directory is missing: ${f}" >&2
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
  done

  # Include every declarative or executable input that can change the prepared
  # cluster. Image identities are added below because a local tag may be
  # rebuilt without changing its text.
  if ! find "${CONF_ROOT}" "${K8S_DEPLOY_ROOT}" -type f \( -name "*.yaml" -o -name "*.yml" \) \
    > "${file_list}"; then
    rm -f "${file_list}" "${hash_input}"
    return 1
  fi
  for f in \
    "${PORTS_CONFIG}" \
    "${DEPLOY_SCRIPT}" \
    "${APPLY_ALL_SCRIPT}" \
    "${TEST_CLIENT_PEER_IDENTITY}" \
    "${GENERATE_CERTS_SCRIPT}" \
    "${BASH_SOURCE[0]}"; do
    if [[ ! -f "${f}" ]]; then
      echo "prepare hash input file is missing: ${f}" >&2
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
    if ! printf '%s\n' "${f}" >> "${file_list}"; then
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
  done
  for f in "${IMAGE_CONFIG_FILE}" "${SCRIPT_DIR}/cluster-patch.sh"; do
    if [[ -f "${f}" ]] && ! printf '%s\n' "${f}" >> "${file_list}"; then
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
  done
  if local_test_binary_mode; then
    if ! printf '%s\n' "${LOCAL_TEST_BIN_DIR}/test_client" "${LOCAL_TEST_BIN_DIR}/test_server" >> "${file_list}"; then
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
  fi
  if local_core_binary_mode; then
    if ! printf '%s\n' "${LOCAL_CORE_BIN_DIR}/edgion-controller" "${LOCAL_CORE_BIN_DIR}/edgion-gateway" >> "${file_list}"; then
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
  fi
  if ! sort -u -o "${file_list}" "${file_list}"; then
    rm -f "${file_list}" "${hash_input}"
    return 1
  fi

  if ! printf '%s\n' \
    "test_server_replicas=${TEST_SERVER_REPLICAS}" \
    "backend_test_namespace=${BACKEND_TEST_NAMESPACE}" \
    "conf_root=${CONF_ROOT}" \
    "deploy_root=${K8S_DEPLOY_ROOT}" \
    "local_image_mode=${LOCAL_IMAGE_MODE_RESOLVED}" \
    "pull_policy=${PULL_POLICY_OVERRIDE}" > "${hash_input}"; then
    rm -f "${file_list}" "${hash_input}"
    return 1
  fi
  for identity in \
    "controller_image:${CONTROLLER_IMAGE}" \
    "gateway_image:${GATEWAY_IMAGE}" \
    "test_client_image:${TEST_CLIENT_IMAGE}" \
    "test_server_image:${TEST_SERVER_IMAGE}"; do
    f="${identity#*:}"
    identity="${identity%%:*}=$(prepare_image_identity "${f}")" || {
      rm -f "${file_list}" "${hash_input}"
      return 1
    }
    if ! printf '%s\n' "${identity}" >> "${hash_input}"; then
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
  done
  if local_test_binary_mode; then
    identity="$(prepare_image_identity "${LOCAL_TEST_RUNTIME_IMAGE}")" || {
      rm -f "${file_list}" "${hash_input}"
      return 1
    }
    if ! printf 'local_test_runtime_image=%s\n' "${identity}" >> "${hash_input}"; then
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
  fi
  while IFS= read -r f; do
    if [[ ! -r "${f}" ]]; then
      echo "prepare hash input file is missing or unreadable: ${f}" >&2
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
    digest="$(hash_file "${f}")" || {
      rm -f "${file_list}" "${hash_input}"
      return 1
    }
    if ! printf '%s:%s\n' "${f}" "${digest}" >> "${hash_input}"; then
      rm -f "${file_list}" "${hash_input}"
      return 1
    fi
  done < "${file_list}"

  result="$(hash_file "${hash_input}")" || {
    rm -f "${file_list}" "${hash_input}"
    return 1
  }
  rm -f "${file_list}" "${hash_input}"
  printf '%s\n' "${result}"
}

prepare_image_identity() {
  local image_ref="$1"
  local image_id=""
  if [[ -z "${image_ref}" ]]; then
    echo "<unset>"
    return 0
  fi
  if [[ "${LOCAL_IMAGE_MODE_RESOLVED}" != "remote-pull" ]] && command -v docker >/dev/null 2>&1; then
    image_id="$(docker image inspect --format '{{.Id}}' "${image_ref}" 2>/dev/null || true)"
  fi
  echo "${image_ref}@${image_id:-<unresolved>}"
}

prepare_images_cacheable() {
  local image_ref
  local images=("${CONTROLLER_IMAGE}" "${GATEWAY_IMAGE}")
  if local_test_binary_mode; then
    images+=("${LOCAL_TEST_RUNTIME_IMAGE}")
  else
    images+=("${TEST_CLIENT_IMAGE}" "${TEST_SERVER_IMAGE}")
  fi

  if [[ "${LOCAL_IMAGE_MODE_RESOLVED}" == "remote-pull" ]]; then
    for image_ref in "${images[@]}"; do
      [[ "${image_ref}" =~ @sha256:[0-9a-fA-F]{64}$ ]] || return 1
    done
    return 0
  fi

  command -v docker >/dev/null 2>&1 || return 1
  for image_ref in "${images[@]}"; do
    docker image inspect "${image_ref}" >/dev/null 2>&1 || return 1
  done
}

write_prepare_state_marker() {
  local conf_hash="$1"
  local now_utc test_client_peer_uid test_client_peer_ip
  now_utc="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  IFS=$'\t' read -r test_client_peer_uid test_client_peer_ip \
    < <("${TEST_CLIENT_PEER_IDENTITY}" current)

  kubectl -n "${STATE_NAMESPACE}" create configmap "${STATE_CONFIGMAP_NAME}" \
    --from-literal=prepare_ok=true \
    --from-literal=state_owner="${PREPARE_STATE_OWNER}" \
    --from-literal=conf_hash="${conf_hash}" \
    --from-literal=test_server_replicas="${TEST_SERVER_REPLICAS}" \
    --from-literal=backend_test_namespace="${BACKEND_TEST_NAMESPACE}" \
    --from-literal=test_client_peer_uid="${test_client_peer_uid}" \
    --from-literal=test_client_peer_ip="${test_client_peer_ip}" \
    --from-literal=updated_at="${now_utc}" \
    --dry-run=client -o yaml | kubectl apply -f -
}

invalidate_prepare_state_marker() {
  kubectl -n "${STATE_NAMESPACE}" delete configmap "${STATE_CONFIGMAP_NAME}" \
    --ignore-not-found=true --wait=true
}

get_state_data() {
  local key="$1"
  kubectl -n "${STATE_NAMESPACE}" get configmap "${STATE_CONFIGMAP_NAME}" -o "jsonpath={.data.${key}}" 2>/dev/null || true
}

verify_service_manifest_ports() {
  local namespace="$1"
  local service_name="$2"
  local manifest="$3"
  local desired live missing

  desired="$(python3 - "${manifest}" "${service_name}" <<'PY'
import sys
import yaml

with open(sys.argv[1], encoding="utf-8") as stream:
    for document in yaml.safe_load_all(stream):
        if not isinstance(document, dict) or document.get("kind") != "Service":
            continue
        if (document.get("metadata") or {}).get("name") != sys.argv[2]:
            continue
        for port in (document.get("spec") or {}).get("ports") or []:
            print("\t".join(str(port.get(key, "")) for key in ("name", "port", "targetPort", "protocol")))
PY
)"
  live="$(kubectl -n "${namespace}" get service "${service_name}" -o json | jq -r \
    '.spec.ports[] | [.name, (.port|tostring), (.targetPort|tostring), .protocol] | @tsv')"
  missing="$(comm -23 \
    <(printf '%s\n' "${desired}" | sed '/^$/d' | sort -u) \
    <(printf '%s\n' "${live}" | sed '/^$/d' | sort -u))"
  if [[ -n "${missing}" ]]; then
    echo "--skip-prepare denied: service ${namespace}/${service_name} is missing prepared ports:" >&2
    printf '%s\n' "${missing}" >&2
    return 1
  fi
}

verify_prepared_runtime_readiness() {
  local namespace
  for namespace in edgion-system edgion-test edgion-default "${BACKEND_TEST_NAMESPACE}" edgion-deleg-dyn edgion-grpc-deleg-dyn; do
    case "${namespace}" in
      edgion-system)
        kubectl rollout status deployment/edgion-controller -n "${namespace}" --timeout=120s >/dev/null || return 1
        kubectl rollout status deployment/edgion-gateway -n "${namespace}" --timeout=120s >/dev/null || return 1
        ;;
      edgion-test)
        kubectl rollout status deployment/edgion-test-client -n "${namespace}" --timeout=120s >/dev/null || return 1
        kubectl rollout status deployment/edgion-test-server -n "${namespace}" --timeout=120s >/dev/null || return 1
        kubectl rollout status deployment/edgion-test-openldap-tls -n "${namespace}" --timeout=120s >/dev/null || return 1
        ;;
      *)
        kubectl rollout status deployment/edgion-test-server -n "${namespace}" --timeout=120s >/dev/null || return 1
        ;;
    esac
  done

  "${ENDPOINT_INVENTORY}" --namespace edgion-test --service edgion-test-server \
    --port 30001 --expected-count "${TEST_SERVER_REPLICAS}" --timeout-secs 30 >/dev/null || return 1
  "${ENDPOINT_INVENTORY}" --namespace edgion-deleg-dyn --service child-dyn-svc \
    --port 30001 --expected-count 1 --timeout-secs 30 >/dev/null || return 1
  "${ENDPOINT_INVENTORY}" --namespace edgion-grpc-deleg-dyn --service child-dyn-grpc-svc \
    --port 30021 --expected-count 1 --timeout-secs 30 >/dev/null || return 1
  kubectl -n edgion-test get secret backend-server-wildcard-tls >/dev/null || return 1
  verify_service_manifest_ports edgion-system edgion-gateway \
    "${K8S_DEPLOY_ROOT}/gateway/service.yaml" || return 1
  verify_service_manifest_ports edgion-test edgion-test-server \
    "${K8S_DEPLOY_ROOT}/test/test-server/service.yaml" || return 1
  "${TEST_CLIENT_PEER_IDENTITY}" assert >/dev/null || return 1
}

verify_skip_prepare_preconditions() {
  local conf_hash expected actual current_peer_uid current_peer_ip
  if ! conf_hash="$(compute_prepare_conf_hash)"; then
    echo "--skip-prepare denied: failed to compute the complete prepare input hash" >&2
    exit 1
  fi

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

  actual="$(get_state_data state_owner)"
  if [[ "${actual}" != "${PREPARE_STATE_OWNER}" ]]; then
    echo "--skip-prepare denied: state marker is not owned by this K8s integration harness"
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

  IFS=$'\t' read -r current_peer_uid current_peer_ip \
    < <("${TEST_CLIENT_PEER_IDENTITY}" current)
  expected="${current_peer_uid}"
  actual="$(get_state_data test_client_peer_uid)"
  if [[ -z "${actual}" || "${actual}" != "${expected}" ]]; then
    echo "--skip-prepare denied: test-client Pod UID drifted" >&2
    exit 1
  fi
  expected="${current_peer_ip}"
  actual="$(get_state_data test_client_peer_ip)"
  if [[ -z "${actual}" || "${actual}" != "${expected}" ]]; then
    echo "--skip-prepare denied: test-client Pod IP drifted" >&2
    exit 1
  fi

  if ! verify_prepared_runtime_readiness; then
    echo "--skip-prepare denied: prepared runtime readiness check failed" >&2
    exit 1
  fi

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
  if kubectl get deployment edgion-test-openldap-tls -n edgion-test >/dev/null 2>&1; then
    echo "Waiting for edgion-test/edgion-test-openldap-tls startup..."
    kubectl rollout status deployment/edgion-test-openldap-tls -n edgion-test --timeout=300s
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
  patch_local_binary_deployment "edgion-deleg-dyn" "edgion-test-server" "test-server"
  patch_local_binary_deployment "edgion-grpc-deleg-dyn" "edgion-test-server" "test-server"
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
    "edgion-deleg-dyn edgion-test-server" \
    "edgion-grpc-deleg-dyn edgion-test-server" \
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
    args="${args} --https-backend-port=30051 --https-backend-mtls-port=30052 --cert-file=/etc/edgion/backend-tls/tls.crt --key-file=/etc/edgion/backend-tls/tls.key --client-ca-file=/etc/edgion/client-ca/ca.crt --https-backend-multi-sni-port=30026 --multi-sni-cert-a=/etc/edgion/backend-sni-a/tls.crt --multi-sni-key-a=/etc/edgion/backend-sni-a/tls.key --multi-sni-cert-b=/etc/edgion/backend-sni-b/tls.crt --multi-sni-key-b=/etc/edgion/backend-sni-b/tls.key --https-backend-wildcard-port=30028 --wildcard-cert=/etc/edgion/backend-wildcard/tls.crt --wildcard-key=/etc/edgion/backend-wildcard/tls.key"
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
  copy_local_binary_to_pods "edgion-deleg-dyn" "app=edgion-test-server" "test-server" "test_server"
  copy_local_binary_to_pods "edgion-grpc-deleg-dyn" "app=edgion-test-server" "test-server" "test_server"
  copy_local_binary_to_pods "${BACKEND_TEST_NAMESPACE}" "app=edgion-test-server" "test-server" "test_server"
  if kubectl get deployment lb-multi-test-server -n edgion-default >/dev/null 2>&1; then
    copy_local_binary_to_pods "edgion-default" "app=edgion-test-server-multi" "test-server" "test_server"
  fi

  start_local_test_server_pods "edgion-test" "app=edgion-test-server" "true"
  start_local_test_server_pods "edgion-default" "app=edgion-test-server" "false"
  start_local_test_server_pods "edgion-deleg-dyn" "app=edgion-test-server" "false"
  start_local_test_server_pods "edgion-grpc-deleg-dyn" "app=edgion-test-server" "false"
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
  local controller_pod pf_pid sid local_port pf_log attempt
  controller_pod="$(kubectl get pods -n edgion-system -l app=edgion-controller \
    --field-selector=status.phase=Running \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
  if [[ -z "${controller_pod}" ]]; then
    echo "unknown"
    return 0
  fi

  pf_log="$(mktemp)"
  kubectl port-forward -n edgion-system "pod/${controller_pod}" :12101 >"${pf_log}" 2>&1 &
  pf_pid="$!"
  _PF_PIDS+=("${pf_pid}")

  local_port=""
  for attempt in $(seq 1 20); do
    local_port="$(sed -n 's/.*Forwarding from 127\.0\.0\.1:\([0-9]\{1,\}\) ->.*/\1/p' "${pf_log}" 2>/dev/null | head -n 1)"
    [[ -n "${local_port}" ]] && break
    sleep 0.5
  done

  if [[ -n "${local_port}" ]]; then
    sid="$(curl -fsS -H "Authorization: Bearer ${CONTROLLER_ADMIN_TOKEN}" \
      "http://127.0.0.1:${local_port}/api/v1/server-info" 2>/dev/null \
      | jq -r '.data.server_id // .server_id // "unknown"' || true)"
  else
    sid="unknown"
  fi

  kill "${pf_pid}" >/dev/null 2>&1 || true
  wait "${pf_pid}" 2>/dev/null || true
  rm -f "${pf_log}"
  echo "${sid:-unknown}"
}

# Fetch the Gateway's current server_id and readiness state.
#
# The Gateway admin API (port 12001) is loopback-only and dropped from the Gateway
# Service, so it cannot be reached over Service DNS from the host. The Gateway image
# ships no curl/wget either, so a `kubectl exec ... curl` is not possible. Instead we
# open a short-lived port-forward to the Gateway Pod and curl it on 127.0.0.1.
# Gateway is replicas:1 and this suite is sequential. We request an ephemeral local
# port (`:12001`) and parse the chosen port from the port-forward output so a leaked
# forward from a prior run cannot cause a fixed-port collision. The PID is tracked in
# `_PF_PIDS` so the top-level trap reaps it even on interrupt.
get_gateway_server_info() {
  local gw_pod pf_pid info last_info local_port pf_log deadline last_error attempt
  last_info=$'unknown\tfalse'
  last_error="probe not started"
  deadline=$((SECONDS + GATEWAY_SYNC_READY_TIMEOUT_SECONDS))

  # Rollout completion and Pod discovery are not atomic. Retry the complete
  # Pod -> port-forward -> server-info chain under one bounded deadline so a
  # transient empty list or a forward that exits before the admin socket opens
  # cannot become an immediate false-negative.
  while (( SECONDS < deadline )); do
    gw_pod="$(kubectl get pods -n edgion-system -l app=edgion-gateway \
      --field-selector=status.phase=Running \
      -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
    if [[ -z "${gw_pod}" ]]; then
      last_error="no running Gateway Pod"
      sleep 0.5
      continue
    fi

    pf_log="$(mktemp)"
    kubectl port-forward -n edgion-system "pod/${gw_pod}" :12001 >"${pf_log}" 2>&1 &
    pf_pid="$!"
    local_port=""
    # Do not use `for attempt in $(seq ...)` here. This function is called from
    # an `IFS=$'\t' read ... <<< "$(...)"` expansion, so newline-based word
    # splitting is intentionally disabled in this subshell.
    for ((attempt = 1; attempt <= 20; attempt++)); do
      local_port="$(sed -n 's/.*Forwarding from 127\.0\.0\.1:\([0-9]\{1,\}\) ->.*/\1/p' "${pf_log}" 2>/dev/null | head -n 1)"
      [[ -n "${local_port}" ]] && break
      kill -0 "${pf_pid}" >/dev/null 2>&1 || break
      sleep 0.5
    done

    if [[ -z "${local_port}" ]]; then
      last_error="$(tr '\n' ' ' < "${pf_log}" 2>/dev/null || true)"
      [[ -n "${last_error}" ]] || last_error="port-forward did not publish a local port"
      kill "${pf_pid}" >/dev/null 2>&1 || true
      wait "${pf_pid}" 2>/dev/null || true
      rm -f "${pf_log}"
      sleep 0.5
      continue
    fi

    while (( SECONDS < deadline )); do
      info="$(curl -fsS -H "Authorization: Bearer ${GATEWAY_ADMIN_TOKEN}" \
        "http://127.0.0.1:${local_port}/api/v1/server-info" 2>/dev/null \
        | jq -r '[.data.server_id // .server_id // "unknown", .data.ready // .ready // false] | @tsv' || true)"
      if [[ -n "${info}" ]]; then
        last_info="${info}"
      fi
      if [[ "${last_info}" != $'unknown\tfalse' && "${last_info##*$'\t'}" == "true" ]]; then
        kill "${pf_pid}" >/dev/null 2>&1 || true
        wait "${pf_pid}" 2>/dev/null || true
        rm -f "${pf_log}"
        echo "${last_info}"
        return 0
      fi
      if ! kill -0 "${pf_pid}" >/dev/null 2>&1; then
        last_error="$(tr '\n' ' ' < "${pf_log}" 2>/dev/null || true)"
        [[ -n "${last_error}" ]] || last_error="port-forward exited before Gateway became ready"
        break
      fi
      last_error="Gateway server-info is not ready"
      sleep 0.5
    done

    kill "${pf_pid}" >/dev/null 2>&1 || true
    wait "${pf_pid}" 2>/dev/null || true
    rm -f "${pf_log}"
    sleep 0.5
  done

  echo "Gateway server-info probe timed out after ${GATEWAY_SYNC_READY_TIMEOUT_SECONDS}s: last=${last_info} error=${last_error}" >&2
  echo "${last_info}"
}

get_gateway_server_id() {
  get_gateway_server_info | cut -f1
}

get_gateway_pod_uid() {
  kubectl get pods -n edgion-system -l app=edgion-gateway \
    --field-selector=status.phase=Running \
    -o jsonpath='{.items[0].metadata.uid}' 2>/dev/null || true
}

restart_gateway_after_initial_fixtures() {
  local before_uid after_uid server_id gateway_ready

  before_uid="$(get_gateway_pod_uid)"
  if [[ -z "${before_uid}" ]]; then
    echo "Post-fixture Gateway restart cannot verify the pre-restart Pod UID"
    return 1
  fi

  echo "Restarting Gateway after initial fixtures (pod_uid=${before_uid})..."
  kubectl rollout restart deployment/edgion-gateway -n edgion-system
  kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=300s
  install_local_core_binaries
  wait_gateway_stable 300

  after_uid="$(get_gateway_pod_uid)"
  if [[ -z "${after_uid}" || "${after_uid}" == "${before_uid}" ]]; then
    echo "Post-fixture Gateway restart did not replace the Pod UID: ${after_uid:-<empty>}"
    return 1
  fi
  IFS=$'\t' read -r server_id gateway_ready <<< "$(get_gateway_server_info)"
  if [[ -z "${server_id}" || "${server_id}" == "unknown" || "${gateway_ready}" != "true" ]]; then
    echo "Post-fixture Gateway replacement did not complete Controller synchronization: server_id=${server_id:-<empty>} ready=${gateway_ready:-<empty>}"
    return 1
  fi

  echo "Post-fixture Gateway restart converged: before_uid=${before_uid} after_uid=${after_uid} server_id=${server_id} ready=${gateway_ready}"
}

trigger_reload() {
  local controller_pod pf_pid local_port pf_log attempt resp
  controller_pod="$(kubectl get pods -n edgion-system -l app=edgion-controller \
    --field-selector=status.phase=Running \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
  if [[ -z "${controller_pod}" ]]; then
    echo "reload api unavailable: no running Controller pod"
    return 1
  fi

  pf_log="$(mktemp)"
  kubectl port-forward -n edgion-system "pod/${controller_pod}" :12101 >"${pf_log}" 2>&1 &
  pf_pid="$!"
  _PF_PIDS+=("${pf_pid}")
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
    echo "reload api port-forward failed"
    return 1
  fi

  resp="$(curl -fsS -X POST -H "Authorization: Bearer ${CONTROLLER_ADMIN_TOKEN}" \
    "http://127.0.0.1:${local_port}/api/v1/reload" 2>/dev/null || true)"
  kill "${pf_pid}" >/dev/null 2>&1 || true
  wait "${pf_pid}" 2>/dev/null || true
  rm -f "${pf_log}"
  if [[ -z "${resp}" ]]; then
    echo "reload api returned empty response"
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

  local max_attempts=1

  local attempt=1
  while (( attempt <= max_attempts )); do
    if [[ "${resource}" == "LinkSys" && "${item}" == "CredentialSourceOAuth2" ]]; then
      if CONF_ROOT="$CONF_ROOT" "$CREDENTIAL_SOURCE_OAUTH2_SCRIPT"; then
        return 0
      fi
    elif [[ "${resource}" == "LinkSys" && "${item}" == "CredentialSourceOAuth2Memory" ]]; then
      if CONF_ROOT="$CONF_ROOT" "$CREDENTIAL_SOURCE_OAUTH2_MEMORY_SCRIPT"; then
        return 0
      fi
    elif [[ "${resource}" == "EdgionBackend" && "${item}" == "PolicyIntegration" ]]; then
      if run_ai_backend_policy_integration; then
        return 0
      fi
    elif "${RUN_CLIENT_SCRIPT}" -r "${resource}" -i "${item}"; then
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
  [[ "${resource}" == "Gateway" && "${item}" == "PortConflict" ]] \
    || [[ "${resource}" == "EdgionBackend" && "${item}" == "PolicyIntegration" ]] \
    || [[ "${resource}" == "LinkSys" && "${item}" == "CredentialSourceOAuth2" ]] \
    || [[ "${resource}" == "LinkSys" && "${item}" == "CredentialSourceOAuth2Memory" ]]
}

run_ai_backend_policy_integration() {
  local expected_replicas test_server_endpoint_ips
  expected_replicas="$(
    endpoint_inventory_strict_deployment_replicas edgion-test edgion-test-server
  )"
  test_server_endpoint_ips="$(
    "${ENDPOINT_INVENTORY}" --namespace edgion-test \
      --service edgion-test-server --port 30001 \
      --expected-count "${expected_replicas}" --timeout-secs 30 --output ips
  )"

  EDGION_TEST_GATEWAY_ADMIN_URL="http://edgion-gateway.edgion-system.svc.cluster.local:12001" \
    EDGION_TEST_AI_PROVIDER_HTTP_BASE="http://edgion-test-server.edgion-test.svc.cluster.local:30001" \
    EDGION_TEST_AI_PROVIDER_HTTPS_BASE="https://edgion-test-server.edgion-test.svc.cluster.local:30051" \
    EDGION_TEST_AI_PROVIDER_MTLS_BASE="https://edgion-test-server.edgion-test.svc.cluster.local:30052" \
    EDGION_TEST_AI_PROVIDER_SNI_BASE="https://edgion-test-server.edgion-test.svc.cluster.local:30026" \
    EDGION_TEST_AI_GATEWAY_NAME="policy-integration-gateway" \
    EDGION_TEST_BACKEND_ENDPOINT_IPS="${test_server_endpoint_ips}" \
    "${RUN_CLIENT_SCRIPT}" --dynamic-scenario AiBackendPolicyIntegration
}

run_manifest_dynamic_scenarios() {
  local expected_file output_file protocol_file
  local dynamic_output dynamic_rc result_key result_status

  expected_file="$(mktemp /tmp/edgion-k8s-dynamic-expected.XXXXXX)"
  output_file="$(mktemp /tmp/edgion-k8s-dynamic-output.XXXXXX)"
  protocol_file="$(mktemp /tmp/edgion-k8s-dynamic-protocol.XXXXXX)"

  awk -F '\t' '
    /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
    { print "Dynamic|" $1 }
  ' "${K8S_DYNAMIC_MANIFEST}" > "${expected_file}"

  dynamic_output="$(
    CONF_ROOT="${CONF_ROOT}" DYNAMIC_MANIFEST="${K8S_DYNAMIC_MANIFEST}" \
      "${RUN_DYNAMIC_SCRIPT}" 2>&1
  )" && dynamic_rc=0 || dynamic_rc=$?
  printf '%s\n' "${dynamic_output}"
  printf '%s\n' "${dynamic_output}" > "${output_file}"
  if [[ -n "${K8S_RUN_DIR:-}" ]]; then
    mkdir -p "${K8S_RUN_DIR}/dynamic"
    printf '%s\n' "${dynamic_output}" > "${K8S_RUN_DIR}/dynamic/${CURRENT_ROUND:-round-1}.log"
  fi

  awk '
    $1 == "@@DYNAMIC_RESULT" {
      print "@@SUITE_RESULT Dynamic|" $2 " " $3 " " $4
      next
    }
    $1 == "@@DYNAMIC_SUMMARY" {
      print "@@BATCH_SUMMARY " $2 " " $3
    }
  ' "${output_file}" > "${protocol_file}"

  SELECTED_COUNT=$((SELECTED_COUNT + $(wc -l < "${expected_file}" | tr -d ' ')))
  if ! edgion_validate_k8s_batch_results "${expected_file}" "${protocol_file}" "${dynamic_rc}"; then
    echo "Dynamic execution returned an incomplete or invalid result protocol"
    rm -f "${expected_file}" "${output_file}" "${protocol_file}"
    return 1
  fi

  EXECUTED_COUNT=$((EXECUTED_COUNT + EDGION_K8S_BATCH_TOTAL))
  FAIL_COUNT=$((FAIL_COUNT + EDGION_K8S_BATCH_FAILED))
  while IFS= read -r line; do
    result_key="$(printf '%s\n' "${line}" | awk '{print $2}')"
    result_status="$(printf '%s\n' "${line}" | awk '{print $3}')"
    if [[ "${result_status}" == "FAIL" ]]; then
      FAILED_SUITES+=("Dynamic/${result_key}")
    fi
  done < <(grep '^@@DYNAMIC_RESULT ' "${output_file}")

  rm -f "${expected_file}" "${output_file}" "${protocol_file}"
}

run_isolated_helper_group() {
  local group_id="$1"
  local expected_file="$2"
  local output_file="${K8S_RUN_DIR}/isolated/${group_id}.log"
  local protocol_file="${K8S_RUN_DIR}/isolated/${group_id}.protocol"
  local helper_rc result_key result_status
  shift 2

  SELECTED_COUNT=$((SELECTED_COUNT + $(wc -l < "${expected_file}" | tr -d ' ')))
  set +e
  "$@" 2>&1 | tee "${output_file}"
  helper_rc=${PIPESTATUS[0]}
  set -e

  if ! awk '
    $1 == "@@ISOLATED_RESULT" {
      if (NF != 4 || $2 !~ /^[A-Za-z][A-Za-z0-9_]*$/ \
          || $3 !~ /^(PASS|FAIL)$/ || $4 !~ /^[0-9]+$/) {
        exit 2
      }
      print "@@SUITE_RESULT Isolated|" $2 " " $3 " " $4
      next
    }
    $1 == "@@ISOLATED_SUMMARY" {
      if (NF != 4 || $2 !~ /^total=[0-9]+$/ || $3 !~ /^failed=[0-9]+$/ \
          || $4 !~ /^infrastructure_failed=[01]$/) {
        exit 2
      }
      if ($4 != "infrastructure_failed=0") {
        exit 3
      }
      print "@@BATCH_SUMMARY " $2 " " $3
      next
    }
  ' "${output_file}" > "${protocol_file}"; then
    echo "Isolated ${group_id} execution returned a malformed protocol or infrastructure failure"
    return 1
  fi

  if ! edgion_validate_k8s_batch_results "${expected_file}" "${protocol_file}" "${helper_rc}"; then
    echo "Isolated ${group_id} execution returned an incomplete or invalid result protocol"
    return 1
  fi

  EXECUTED_COUNT=$((EXECUTED_COUNT + EDGION_K8S_BATCH_TOTAL))
  FAIL_COUNT=$((FAIL_COUNT + EDGION_K8S_BATCH_FAILED))
  while IFS= read -r line; do
    result_key="$(printf '%s\n' "${line}" | awk '{print $2}')"
    result_status="$(printf '%s\n' "${line}" | awk '{print $3}')"
    if [[ "${result_status}" == "FAIL" ]]; then
      FAILED_SUITES+=("Isolated/${result_key}")
    fi
  done < <(grep '^@@ISOLATED_RESULT ' "${output_file}")
}

run_all_isolated_scenarios() {
  local jobs_expected="${K8S_RUN_DIR}/isolated/jobs.expected"
  local proxy_expected="${K8S_RUN_DIR}/isolated/proxy.expected"
  local isolated_harness_failed=false
  mkdir -p "${K8S_RUN_DIR}/isolated/jobs" "${K8S_RUN_DIR}/isolated/proxy"

  # suite-parity: isolated controller_auth
  # suite-parity: isolated gateway_admin_auth
  # suite-parity: isolated fips_policy
  # suite-parity: isolated geoip_db_states
  # suite-parity: isolated plugin_policy
  # suite-parity: isolated unknown_config_fields
  printf '%s\n' \
    'Isolated|controller_auth' \
    'Isolated|gateway_admin_auth' \
    'Isolated|fips_policy' \
    'Isolated|geoip_db_states' \
    'Isolated|plugin_policy' \
    'Isolated|unknown_config_fields' > "${jobs_expected}"

  run_isolated_helper_group jobs "${jobs_expected}" \
    "${RUN_ISOLATED_JOBS_SCRIPT}" \
    --image "${ISOLATED_JOB_IMAGE}" \
    --image-pull-policy "${ISOLATED_IMAGE_PULL_POLICY}" \
    --deadline-grace-seconds "${ISOLATED_DEADLINE_GRACE_SECONDS}" \
    --artifacts-dir "${K8S_RUN_DIR}/isolated/jobs" \
    --job controller_auth \
    --job gateway_admin_auth \
    --job fips_policy \
    --job geoip_db_states \
    --job plugin_policy \
    --job unknown_config_fields \
    || isolated_harness_failed=true

  # suite-parity: isolated proxy_protocol_inbound
  # suite-parity: isolated proxy_protocol_chain
  printf '%s\n' \
    'Isolated|proxy_protocol_inbound' \
    'Isolated|proxy_protocol_chain' > "${proxy_expected}"

  run_isolated_helper_group proxy "${proxy_expected}" \
    "${RUN_PROXY_PROTOCOL_JOBS_SCRIPT}" \
    --image "${ISOLATED_JOB_IMAGE}" \
    --haproxy-image "${HAPROXY_IMAGE}" \
    --image-pull-policy "${ISOLATED_IMAGE_PULL_POLICY}" \
    --haproxy-pull-policy "${HAPROXY_IMAGE_PULL_POLICY}" \
    --timeout-seconds "${PROXY_TOPOLOGY_TIMEOUT_SECONDS}" \
    --artifacts-dir "${K8S_RUN_DIR}/isolated/proxy" \
    --job proxy_protocol_inbound \
    --job proxy_protocol_chain \
    || isolated_harness_failed=true

  [[ "${isolated_harness_failed}" == "false" ]]
}

run_final_gate() {
  local gate_id="$1"
  local gate_helper="$2"
  local expected_file output_file protocol_file
  local gate_output gate_rc result_status protocol_parse_failed=false
  shift 2

  expected_file="$(mktemp /tmp/edgion-k8s-final-expected.XXXXXX)"
  output_file="$(mktemp /tmp/edgion-k8s-final-output.XXXXXX)"
  protocol_file="$(mktemp /tmp/edgion-k8s-final-protocol.XXXXXX)"
  printf 'Final|%s\n' "${gate_id}" > "${expected_file}"

  gate_output="$("${gate_helper}" "$@" 2>&1)" && gate_rc=0 || gate_rc=$?
  printf '%s\n' "${gate_output}"
  printf '%s\n' "${gate_output}" > "${output_file}"
  if [[ -n "${K8S_RUN_DIR:-}" ]]; then
    mkdir -p "${K8S_RUN_DIR}/final"
    printf '%s\n' "${gate_output}" > "${K8S_RUN_DIR}/final/${gate_id}.log"
  fi

  if ! awk -v expected_gate="${gate_id}" '
    $1 == "@@FINAL_GATE_RESULT" {
      if ($2 != expected_gate) {
        exit 2
      }
      if (expected_gate == "MetricsLabelWhitelist") {
        if (NF != 5 || $4 !~ /^endpoints=[0-9]+$/ || $5 !~ /^checked=[0-9]+$/) {
          exit 2
        }
      } else if (expected_gate == "UnknownConfigFields") {
        if (NF != 6 || $4 !~ /^total=([0-9]+|unknown)$/ \
            || $5 !~ /^items=([0-9]+|unknown)$/ \
            || $6 !~ /^truncated=(true|false|unknown)$/) {
          exit 2
        }
      } else if (expected_gate == "RequeueDeadLoop") {
        if (NF != 6 || $4 !~ /^pods=[0-9]+$/ || $5 !~ /^streams=[0-9]+$/ \
            || $6 !~ /^matches=[0-9]+$/) {
          exit 2
        }
      } else if (expected_gate == "CacheResidue") {
        if (NF != 5 || $4 !~ /^validator_rc=([0-9]+|not-run)$/ \
            || $5 !~ /^reason=[A-Za-z0-9_-]+$/) {
          exit 2
        }
      } else {
        exit 2
      }
      suite_rc = ($3 == "PASS" ? 0 : 1)
      print "@@SUITE_RESULT Final|" $2 " " $3 " " suite_rc
      next
    }
    $1 == "@@FINAL_GATE_SUMMARY" {
      printf "@@BATCH_SUMMARY"
      for (field = 2; field <= NF; field++) {
        printf " %s", $field
      }
      printf "\n"
      next
    }
    $1 ~ /^@@FINAL_GATE_/ {
      exit 2
    }
  ' "${output_file}" > "${protocol_file}"; then
    protocol_parse_failed=true
  fi

  SELECTED_COUNT=$((SELECTED_COUNT + 1))
  if [[ "${protocol_parse_failed}" == "true" ]] \
    || ! edgion_validate_k8s_batch_results "${expected_file}" "${protocol_file}" "${gate_rc}"; then
    echo "Final gate ${gate_id} returned an incomplete or invalid result protocol"
    rm -f "${expected_file}" "${output_file}" "${protocol_file}"
    return 1
  fi

  EXECUTED_COUNT=$((EXECUTED_COUNT + EDGION_K8S_BATCH_TOTAL))
  FAIL_COUNT=$((FAIL_COUNT + EDGION_K8S_BATCH_FAILED))
  result_status="$(awk '$1 == "@@FINAL_GATE_RESULT" { print $3 }' "${output_file}")"
  if [[ "${result_status}" == "FAIL" ]]; then
    FAILED_SUITES+=("Final/${gate_id}")
  fi

  rm -f "${expected_file}" "${output_file}" "${protocol_file}"
}

run_all_final_gates() {
  local final_gate_harness_failed=false

  echo
  echo "=============================="
  echo "Running K8s final validation gates"
  echo "=============================="

  # suite-parity: final-gate MetricsLabelWhitelist
  run_final_gate MetricsLabelWhitelist "${RUN_METRICS_LABEL_GATE}" \
    || final_gate_harness_failed=true

  # suite-parity: final-gate UnknownConfigFields
  run_final_gate UnknownConfigFields "${RUN_UNKNOWN_CONFIG_FIELDS_GATE}" \
    || final_gate_harness_failed=true

  # suite-parity: final-gate RequeueDeadLoop
  run_final_gate RequeueDeadLoop "${RUN_REQUEUE_DEAD_LOOP_GATE}" \
    --run-start-time "${RUN_START_TIME}" || final_gate_harness_failed=true

  # CacheResidue is intentionally last: its validator wipes shared Controller
  # and Gateway state as part of residue detection.
  # suite-parity: final-gate CacheResidue
  run_final_gate CacheResidue "${RUN_CACHE_RESIDUE_GATE}" \
    || final_gate_harness_failed=true

  [[ "${final_gate_harness_failed}" == "false" ]]
}

run_all_selected_suites() {
  local use_start_from="$1"

  # Map: config suite dir|resource|item
  local suites=(
    "HTTPRoute/Basic|HTTPRoute|Basic"
    "HTTPRoute/Match|HTTPRoute|Match"
    "HTTPRoute/Backend|HTTPRoute|Backend"
    "HTTPRoute/Backend/LBRoundRobin|HTTPRoute|Backend/LBRoundRobin"
    "HTTPRoute/Backend/LBConsistentHash|HTTPRoute|Backend/LBConsistentHash"
    "HTTPRoute/Backend/LBLeastConn|HTTPRoute|Backend/LBLeastConn"
    "HTTPRoute/Backend/LBLeastConnRetryOnStatus|HTTPRoute|Backend/LBLeastConnRetryOnStatus"
    "HTTPRoute/Backend/LBEwma|HTTPRoute|Backend/LBEwma"
    "HTTPRoute/Backend/LBEwmaRetryOnStatus|HTTPRoute|Backend/LBEwmaRetryOnStatus"
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
    "HTTPRoute/Backend/DnsResolution|HTTPRoute|Backend/DnsResolution"
    "HTTPRoute/Backend/DnsResolutionFailure|HTTPRoute|Backend/DnsResolutionFailure"
    "HTTPRoute/Backend/DegradeThreshold|HTTPRoute|Backend/DegradeThreshold"
    "HTTPRoute/DanglingExtensionRef|HTTPRoute|DanglingExtensionRef"
    "HTTPRoute/Delegation|HTTPRoute|Delegation"
    "HTTPRoute/Filters|HTTPRoute|Filters"
    "HTTPRoute/Filters/Redirect|HTTPRoute|Filters/Redirect"
    "HTTPRoute/Filters/Security|HTTPRoute|Filters/Security"
    "HTTPRoute/Filters/HeaderModifier|HTTPRoute|Filters/HeaderModifier"
    "HTTPRoute/Filters/FilterConflict|HTTPRoute|Filters/FilterConflict"
    "HTTPRoute/Filters/PercentMirror|HTTPRoute|Filters/PercentMirror"
    "HTTPRoute/Protocol/WebSocket|HTTPRoute|Protocol/WebSocket"
    "HTTPRoute/Protocol/H2C|HTTPRoute|Protocol/H2C"
    "HTTPRoute/Protocol/GrpcWeb|HTTPRoute|Protocol/GrpcWeb"
    "HTTPRoute/Protocol/H2TrailerMalformed|HTTPRoute|Protocol/H2TrailerMalformed"
    "HTTPRoute/SseStreaming|HTTPRoute|SseStreaming"
    "HTTPRoute/Transport|HTTPRoute|Transport"
    "HTTPRoute/MultiPort|HTTPRoute|MultiPort"
    "GRPCRoute/Basic|GRPCRoute|Basic"
    "GRPCRoute/Match|GRPCRoute|Match"
    "GRPCRoute/RateLimit|GRPCRoute|RateLimit"
    "GRPCRoute/Backend/SessionPersistence|GRPCRoute|Backend/SessionPersistence"
    "GRPCRoute/Backend/SessionPersistenceWeighted|GRPCRoute|Backend/SessionPersistenceWeighted"
    "GRPCRoute/Delegation|GRPCRoute|Delegation"
    "GRPCRoute/UnmatchedRoute|GRPCRoute|UnmatchedRoute"
    "TCPRoute/Basic|TCPRoute|Basic"
    "TCPRoute/Basic|TCPRoute|KeepaliveSmoke"
    "TCPRoute/StreamPlugins|TCPRoute|StreamPlugins"
    "TCPRoute/GlobalConnectionIpRestriction|TCPRoute|GlobalConnectionIpRestriction"
    "TCPRoute/IdleTimeout|TCPRoute|IdleTimeout"
    "TCPRoute/GeoRestriction|TCPRoute|GeoRestriction"
    "TLSRoute/Basic|TLSRoute|Basic"
    "TLSRoute/ProxyProtocol|TLSRoute|ProxyProtocol"
    "TLSRoute/StreamPlugins|TLSRoute|StreamPlugins"
    "TLSRoute/MultiSNI|TLSRoute|MultiSNI"
    "TLSRoute/BothAbsentParentRef|TLSRoute|BothAbsentParentRef"
    "TLSRoute/Passthrough|TLSRoute|Passthrough"
    "TLSRoute/UnknownKindBackend|TLSRoute|UnknownKindBackend"
    "TLSRoute/GeoRestriction|TLSRoute|GeoRestriction"
    "UDPRoute/Basic|UDPRoute|Basic"
    "UDPRoute/StreamPlugins|UDPRoute|StreamPlugins"
    "Gateway/Security|Gateway|Security"
    "Gateway/RealIP|Gateway|RealIP"
    "Gateway/TLS/BackendTLS|Gateway|TLS/BackendTLS"
    "Gateway/TLS/BackendTLSGatewayCert|Gateway|TLS/BackendTLSGatewayCert"
    "Gateway/TLS/GatewayTLS|Gateway|TLS/GatewayTLS"
    "Gateway/TLS/MisdirectedRequest|Gateway|TLS/MisdirectedRequest"
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
    "EdgionTls/port_only|EdgionTls|port_only"
    "EdgionTls/sni_collision|EdgionTls|sni_collision"
    "EdgionTls/wildcard_single_label|EdgionTls|wildcard_single_label"
    "EdgionTls/BothAbsentParentRef|EdgionTls|BothAbsentParentRef"
    "EdgionPlugins/DebugAccessLog|EdgionPlugins|DebugAccessLog"
    "EdgionPlugins/AccessLogExtern|EdgionPlugins|AccessLogExtern"
    "EdgionPlugins/AiGuard|EdgionPlugins|AiGuard"
    "EdgionPlugins/BodyPlugins|EdgionPlugins|BodyPlugins"
    "EdgionPlugins/Canary|EdgionPlugins|Canary"
    "EdgionPlugins/CredentialInjector|EdgionPlugins|CredentialInjector"
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
    "EdgionPlugins/IpRestrictionPartialGood|EdgionPlugins|IpRestrictionPartialGood"
    "EdgionPlugins/IpRestrictionRefs|EdgionPlugins|IpRestrictionRefs"
    "EdgionPlugins/JsonSchemaValidation|EdgionPlugins|JsonSchemaValidation"
    "EdgionPlugins/LdapAuthTls|EdgionPlugins|LdapAuthTls"
    "EdgionPlugins/MatchedRouteMeta|EdgionPlugins|MatchedRouteMeta"
    "EdgionPlugins/PathNormalization|EdgionPlugins|PathNormalization"
    "EdgionPlugins/PathNormalization/ForwardView|EdgionPlugins|PathNormalization/ForwardView"
    "EdgionPlugins/PathNormalization/PreserveParam|EdgionPlugins|PathNormalization/PreserveParam"
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
    "EdgionPlugins/PluginValidationStatus|EdgionPlugins|PluginValidationStatus"
    "EdgionPlugins/RequestAccessPolicy|EdgionPlugins|RequestAccessPolicy"
    "EdgionPlugins/TraceContext|EdgionPlugins|TraceContext"
    "EdgionPlugins/Wasm|EdgionPlugins|Wasm"
    "EdgionPlugins/Dsl|EdgionPlugins|Dsl"
    "EdgionPlugins/Dsl|EdgionPlugins|DslGrpcError"
    "EdgionPlugins/DslBody|EdgionPlugins|DslBody"
    "EdgionPlugins/EdgionConfigDataManaged|EdgionPlugins|EdgionConfigDataManaged"
    "EdgionPlugins/GeoRestriction|EdgionPlugins|GeoRestriction"
    "EdgionPlugins/GlobalPluginsSnapshot|EdgionPlugins|GlobalPluginsSnapshot"
    "EdgionPlugins/Guardrail|EdgionPlugins|Guardrail"
    "EdgionPlugins/AllEndpointStatus|EdgionPlugins|AllEndpointStatus"
    "EdgionBackendTrafficPolicy/RetryBudget|EdgionBackendTrafficPolicy|RetryBudget"
    "EdgionBackendTrafficPolicy/ConnectTimeout|EdgionBackendTrafficPolicy|ConnectTimeout"
    "EdgionBackendTrafficPolicy/UpstreamAuthority|EdgionBackendTrafficPolicy|UpstreamAuthority"
    "Status/RefGrant|Status|RefGrant"
    "Status/RefGrant/TCPRouteCrossNsSvc|Status|RefGrant/TCPRouteCrossNsSvc"
    "Status/RefGrant/TLSRouteCrossNsSvc|Status|RefGrant/TLSRouteCrossNsSvc"
    "Status/BackendTLSPolicy|Status|BackendTLSPolicy"
    "Status/EdgionBackendTrafficPolicy|Status|EdgionBackendTrafficPolicy"
    "Status/GatewayFrontendValidation|Status|GatewayFrontendValidation"
    "Status/GRPCRoute|Status|GRPCRoute"
    "Status/HostnameIntersection|Status|HostnameIntersection"
    "Status/RefGrant/EdgionConfigData|Status|RefGrant/EdgionConfigData"
    "Status/TLSListenerMode|Status|TLSListenerMode"
    "Status/TLSRoute|Status|TLSRoute"
    "LinkSys/Status|LinkSys|Status"
    "LinkSys/CredentialSourceOAuth2|LinkSys|CredentialSourceOAuth2"
    "LinkSys/CredentialSourceOAuth2Memory|LinkSys|CredentialSourceOAuth2Memory"
    "EdgionBackend/AnthropicProvider|EdgionBackend|AnthropicProvider"
    "EdgionBackend/CredentialExhaustion|EdgionBackend|CredentialExhaustion"
    "EdgionBackend/Resilience|EdgionBackend|Resilience"
    "EdgionBackend/SingleProvider|EdgionBackend|SingleProvider"
    "EdgionBackend/PolicyIntegration/DynamicTest|EdgionBackend|PolicyIntegration"
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

    local batch_output batch_output_file batch_rc
    batch_output="$("${RUN_CLIENT_BATCH_SCRIPT}" "${batch_tmp}" 2>&1)" && batch_rc=0 || batch_rc=$?
    printf '%s\n' "${batch_output}"
    batch_output_file="$(mktemp /tmp/edgion-batch-output.XXXXXX)"
    printf '%s\n' "${batch_output}" > "${batch_output_file}"
    mkdir -p "${K8S_RUN_DIR}/suites"
    printf '%s\n' "${batch_output}" > "${K8S_RUN_DIR}/suites/${CURRENT_ROUND}-batch.log"
    awk -v output_dir="${K8S_RUN_DIR}/suites" -v round="${CURRENT_ROUND}" '
      $1 == "@@SUITE_START" {
        suite = $2
        gsub(/[^A-Za-z0-9_.-]/, "_", suite)
        output = output_dir "/" round "-" suite ".log"
      }
      output != "" { print >> output }
      $1 == "@@SUITE_RESULT" { close(output); output = "" }
    ' "${batch_output_file}"

    if ! edgion_validate_k8s_batch_results "${batch_tmp}" "${batch_output_file}" "${batch_rc}"; then
      echo "Batch execution returned an incomplete or invalid result protocol"
      HARNESS_FAILED=true
    else
      while IFS= read -r line; do
        local result_key result_status
        result_key="$(printf '%s\n' "${line}" | awk '{print $2}')"
        result_status="$(printf '%s\n' "${line}" | awk '{print $3}')"
        EXECUTED_COUNT=$((EXECUTED_COUNT + 1))
        if [[ "${result_status}" == "FAIL" ]]; then
          FAIL_COUNT=$((FAIL_COUNT + 1))
          FAILED_SUITES+=("${result_key/|//}")
        fi
      done < <(printf '%s\n' "${batch_output}" | grep '^@@SUITE_RESULT ')
    fi

    rm -f "${batch_tmp}" "${batch_output_file}"
  fi

  # Phase 3: Run host-side suites individually.
  for pair in ${host_suites[@]+"${host_suites[@]}"}; do
    local suite_dir rest resource item
    suite_dir="${pair%%|*}"
    rest="${pair#*|}"
    resource="${rest%%|*}"
    item="${rest#*|}"

    local host_log_name
    host_log_name="$(suite_test_name "${resource}" "${item}")"
    mkdir -p "${K8S_RUN_DIR}/suites"
    # run_one is piped through tee and therefore runs in a subshell. Account
    # for the host-side suite here in the parent shell.
    EXECUTED_COUNT=$((EXECUTED_COUNT + 1))
    if ! run_one "${suite_dir}" "${resource}" "${item}" 2>&1 \
      | tee "${K8S_RUN_DIR}/suites/${CURRENT_ROUND}-${host_log_name}.log"; then
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
    if kubectl -n "${STATE_NAMESPACE}" get configmap "${STATE_CONFIGMAP_NAME}" >/dev/null 2>&1 \
      && [[ "$(get_state_data prepare_ok)" == "true" ]] \
      && [[ "$(get_state_data state_owner)" == "${PREPARE_STATE_OWNER}" ]]; then
      REPREPARE_EXISTING=true
    fi
    return 0
  fi
  # Auto-detect: if state ConfigMap exists and matches, skip prepare.
  local conf_hash
  if ! conf_hash="$(compute_prepare_conf_hash)"; then
    echo "Prepare state cannot be reused because its input hash could not be computed." >&2
    return 0
  fi
  if ! kubectl -n "${STATE_NAMESPACE}" get configmap "${STATE_CONFIGMAP_NAME}" >/dev/null 2>&1; then
    return 0
  fi
  local stored_ok stored_owner stored_hash stored_replicas stored_ns
  stored_ok="$(get_state_data prepare_ok)"
  stored_owner="$(get_state_data state_owner)"
  if [[ "${stored_ok}" != "true" || "${stored_owner}" != "${PREPARE_STATE_OWNER}" ]]; then
    echo "Existing prepare state is not owned by this K8s integration harness; refusing in-place refresh." >&2
    return 0
  fi
  REPREPARE_EXISTING=true
  stored_hash="$(get_state_data conf_hash)"
  stored_replicas="$(get_state_data test_server_replicas)"
  stored_ns="$(get_state_data backend_test_namespace)"
  if [[ "${stored_ok}" == "true" \
     && "${stored_hash}" == "${conf_hash}" \
     && "${stored_replicas}" == "${TEST_SERVER_REPLICAS}" \
     && "${stored_ns}" == "${BACKEND_TEST_NAMESPACE}" ]]; then
    if ! prepare_images_cacheable; then
      echo "Prepare state cannot be reused because one or more image references have no immutable identity."
      return 0
    fi
    if verify_prepared_runtime_readiness; then
      return 1
    fi
    echo "Prepared runtime drifted from the cached state; running prepare again."
    return 0
  fi
  return 0
}

preflight_kubernetes_cluster
print_cluster_and_image_context
if [[ "${FILTERED_MODE}" == "false" && "${PREPARE_ONLY}" == "false" && -z "${ISOLATED_JOB_IMAGE}" ]]; then
  echo "EDGION_ISOLATED_JOB_IMAGE or ISOLATED_JOB_IMAGE in --image-config is required for an unfiltered test run"
  exit 1
fi

if should_run_prepare; then
  echo "Phase 1/2: Prepare environment"
  echo "Using BACKEND_TEST_NAMESPACE=${BACKEND_TEST_NAMESPACE}"
  "${VALIDATE_SCRIPT}" "${CONF_ROOT}"
  "${FIXTURE_OWNERSHIP_VALIDATOR}" --fail-on-identical "${CONF_ROOT}"
  if ! PREPARE_CONF_HASH="$(compute_prepare_conf_hash)"; then
    echo "Failed to compute the complete prepare input hash." >&2
    exit 1
  fi

  if [[ "${SKIP_DEPLOY}" == "false" ]]; then
    if [[ "${KEEP_ALIVE}" == "false" && "${REPREPARE_EXISTING}" == "false" ]]; then
      assert_clean_run_boundary
    elif [[ "${REPREPARE_EXISTING}" == "true" ]]; then
      echo "Refreshing the existing prepared integration cluster in place."
      invalidate_prepare_state_marker
    else
      echo "Run-owned cleanup disabled by --keep-alive/--prepare-only"
    fi
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
      "${deploy_args[@]}" 2>&1 | tee "${K8S_RUN_DIR}/deploy.log"
    if local_test_binary_mode; then
      echo "Waiting controller rollout..."
      kubectl rollout status deployment/edgion-controller -n edgion-system --timeout=300s
      echo "Waiting gateway rollout..."
      kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=300s
    fi
    install_local_core_binaries
  fi

  ensure_gatewayclass_compat
  "${APPLY_ALL_SCRIPT}" "${CONF_ROOT}" 2>&1 | tee "${K8S_RUN_DIR}/apply.log"
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

  echo "Publishing canonical test-client peer identity..."
  "${TEST_CLIENT_PEER_IDENTITY}" publish

  restart_gateway_after_initial_fixtures

  force_sync_secrets

  write_prepare_state_marker "${PREPARE_CONF_HASH}"
  echo "Prepare phase finished."
else
  if [[ "${SKIP_PREPARE}" == "true" ]]; then
    echo "Phase 1/2: Skipped (--skip-prepare)"
  else
    echo "Phase 1/2: Skipped (state ConfigMap up-to-date, use --force-prepare to override)"
  fi
  verify_skip_prepare_preconditions
  install_local_core_binaries
fi

if [[ "${PREPARE_ONLY}" == "true" ]]; then
  echo "Prepare-only mode finished successfully. Cluster state is kept."
  K8S_RUN_FINALIZED=true
  exit 0
fi

echo "Phase 2/2: Run tests"
"${TEST_CLIENT_PEER_IDENTITY}" assert
if [[ "${FULL_TEST}" == "true" ]]; then
  echo "Mode: Full test (includes slow suites)"
else
  echo "Mode: Fast test (slow suites skipped; use --full-test to include)"
fi
if [[ "${FILTERED_MODE}" == "false" ]]; then
  echo
  echo "=============================="
  echo "Running isolated K8s scenarios"
  echo "=============================="
  if ! run_all_isolated_scenarios; then
    HARNESS_FAILED=true
  fi
else
  echo "Skip isolated K8s scenarios in filtered mode"
fi
if [[ "${WITH_RELOAD}" == "true" ]]; then
  echo "Rounds: 2 (with reload)"
  c_before="$(get_server_id)"
  g_before="$(get_gateway_server_id)"
  echo "Before reload: controller=${c_before} gateway=${g_before}"

  echo "Test round #1"
  CURRENT_ROUND="round-1"
  run_all_selected_suites "true"

  echo "Triggering reload..."
  trigger_reload

  reload_deadline=$(( $(date +%s) + 120 ))
  c_after="unknown"
  g_after="unknown"
  while (( $(date +%s) < reload_deadline )); do
    c_after="$(get_server_id)"
    g_after="$(get_gateway_server_id)"
    if [[ "${c_after}" != "unknown" && "${g_after}" != "unknown" \
      && "${c_after}" != "${c_before}" && "${g_after}" != "${g_before}" ]]; then
      break
    fi
    sleep 2
  done
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
  CURRENT_ROUND="round-2"
  "${TEST_CLIENT_PEER_IDENTITY}" assert
  run_all_selected_suites "false"
else
  echo "Rounds: 1"
  CURRENT_ROUND="round-1"
  run_all_selected_suites "true"
fi

if [[ "${FILTERED_MODE}" == "false" ]]; then
  echo
  echo "=============================="
  echo "Running manifest-backed K8s dynamic scenarios"
  echo "=============================="
  "${TEST_CLIENT_PEER_IDENTITY}" assert
  if ! run_manifest_dynamic_scenarios; then
    HARNESS_FAILED=true
  fi
else
  echo "Skip manifest-backed dynamic scenarios in filtered mode"
fi

echo
# Validate the suite/dynamic phase before final gates add their own four
# protocol-accounted entries. This preserves the empty-run fail-closed check.
if [[ "${SELECTED_COUNT}" -eq 0 ]]; then
  if [[ "${FILTERED_MODE}" == "true" ]]; then
    echo "No suite matched filter: resource=${ONLY_RESOURCE} item=${ONLY_ITEM:-<all>}"
  else
    echo "No suites were selected by the K8s integration inventory"
  fi
  HARNESS_FAILED=true
fi

if [[ "${MISSING_COUNT}" -gt 0 ]]; then
  echo "Missing K8s suite configuration is a harness failure:"
  for s in "${MISSING_SUITES[@]}"; do
    echo "  - ${s}"
  done
  HARNESS_FAILED=true
fi

if [[ "${EXECUTED_COUNT}" -eq 0 ]]; then
  echo "No suites executed; refusing to report a successful empty run"
  HARNESS_FAILED=true
elif [[ "${EXECUTED_COUNT}" -ne "${SELECTED_COUNT}" ]]; then
  echo "Suite accounting mismatch: selected=${SELECTED_COUNT} executed=${EXECUTED_COUNT}"
  HARNESS_FAILED=true
fi

if [[ "${FILTERED_MODE}" == "false" ]]; then
  if ! run_all_final_gates; then
    HARNESS_FAILED=true
  fi
else
  echo "Skip final validation gates in filtered mode"
fi

if [[ "${EXECUTED_COUNT}" -ne "${SELECTED_COUNT}" ]]; then
  echo "Final accounting mismatch: selected=${SELECTED_COUNT} executed=${EXECUTED_COUNT}"
  HARNESS_FAILED=true
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
echo "  Harness  : $(if [[ "${HARNESS_FAILED}" == "true" ]]; then echo FAIL; else echo PASS; fi)"
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
if [[ "${RUN_OWNS_CLUSTER}" == "true" && "${KEEP_ALIVE}" == "false" ]]; then
  echo "  Run-owned cluster resources will be cleaned after diagnostics."
else
  echo "  Cluster state is kept (not owned by this run or --keep-alive was selected)."
fi
echo "=========================================================="

K8S_RUN_FINALIZED=true
if [[ "${FAIL_COUNT}" -gt 0 || "${HARNESS_FAILED}" == "true" ]]; then
  exit 1
fi
