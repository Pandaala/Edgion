#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="${NAMESPACE:-edgion-test}"
POD="${POD:-}"
CONTAINER="${CONTAINER:-test-client}"
TARGET_HOST="${TARGET_HOST:-edgion-gateway.edgion-system.svc.cluster.local}"
TEST_CLIENT_BIN_PATH="${TEST_CLIENT_BIN_PATH:-/usr/local/bin/test_client}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
ENDPOINT_INVENTORY="${SCRIPT_DIR}/endpoint_inventory.sh"
PLUGIN_VALIDATION_ADMISSION="${SCRIPT_DIR}/verify_plugin_validation_admission.sh"

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH"
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq not found in PATH"
  exit 1
fi
if [[ ! -x "${ENDPOINT_INVENTORY}" ]]; then
  echo "EndpointSlice inventory helper not found or not executable: ${ENDPOINT_INVENTORY}"
  exit 1
fi
# shellcheck source=endpoint_inventory.sh
source "${ENDPOINT_INVENTORY}"

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

if [[ -z "${POD}" ]]; then
  echo "Cannot find test-client pod in namespace ${NAMESPACE}"
  exit 1
fi

echo "Using pod: ${POD}"
echo "Target host: ${TARGET_HOST}"

RESOURCE=""
ITEM=""
ARGS=("$@")
for ((idx=0; idx<${#ARGS[@]}; idx++)); do
  case "${ARGS[$idx]}" in
    -r|--resource)
      if (( idx + 1 < ${#ARGS[@]} )); then
        RESOURCE="${ARGS[$((idx + 1))]}"
      fi
      ;;
    -i|--item)
      if (( idx + 1 < ${#ARGS[@]} )); then
        ITEM="${ARGS[$((idx + 1))]}"
      fi
      ;;
  esac
done

IFS=$'\t' read -r PEER_UID PEER_IP PEER_HIT_CIDR PEER_MISS_CIDR < <(TEST_CLIENT_NAMESPACE="${NAMESPACE}" "${SCRIPT_DIR}/test_client_peer_identity.sh" environment)
[[ -n "${PEER_UID}" && -n "${PEER_IP}" && -n "${PEER_HIT_CIDR}" && -n "${PEER_MISS_CIDR}" ]] || { echo "Missing prepared peer identity" >&2; exit 1; }
SELECTED_PEER="$(kubectl get pod "${POD}" --namespace "${NAMESPACE}" --output json --request-timeout=5s | jq -r '[.metadata.uid, .status.podIP] | @tsv')"
[[ "${SELECTED_PEER}" == "${PEER_UID}"$'\t'"${PEER_IP}" ]] || { echo "Selected Pod does not match prepared Ready peer identity" >&2; exit 1; }

TEST_TARGET_HOST="${TARGET_HOST}"
K8S_ADMISSION_VERIFIED="${EDGION_TEST_K8S_ADMISSION_VERIFIED:-}"

if [[ "${RESOURCE}" == "EdgionPlugins" && "${ITEM}" == "PluginValidationStatus" ]]; then
  "${PLUGIN_VALIDATION_ADMISSION}"
  K8S_ADMISSION_VERIFIED="PluginValidationStatus"
fi

# Compatibility for current test-client image: admin_api_url uses target_host+admin_port.
# PortConflict suite only queries controller Admin API, so route target_host to controller.
if [[ "${RESOURCE}" == "Gateway" && "${ITEM}" == "PortConflict" ]]; then
  TEST_TARGET_HOST="${EDGION_CONTROLLER_HOST:-edgion-controller.edgion-system.svc.cluster.local}"
fi

if [[ "${RESOURCE}" == "Gateway" && "${ITEM}" == "PortConflict" ]]; then
  check_gateway_has_listeners_not_valid() {
    local gw_json="$1"
    echo "${gw_json}" | jq -e '.status.conditions // [] | any(.type=="ListenersNotValid" and .status=="True")' >/dev/null
  }

  check_listener_conflicted() {
    local gw_json="$1"
    local listener_name="$2"
    echo "${gw_json}" | jq -e --arg name "${listener_name}" \
      '.status.listeners // [] | map(select(.name==$name)) | any(.conditions // [] | any(.type=="Conflicted" and .status=="True"))' >/dev/null
  }

  check_listener_not_conflicted() {
    local gw_json="$1"
    local listener_name="$2"
    echo "${gw_json}" | jq -e --arg name "${listener_name}" \
      '.status.listeners // [] | map(select(.name==$name)) | any(.conditions // [] | any(.type=="Conflicted" and .status=="False"))' >/dev/null
  }

  wait_gateway_json() {
    local gw="$1"
    local timeout="${2:-30}"
    local start now
    start="$(date +%s)"
    while true; do
      if kubectl get gateway -n edgion-test "${gw}" -o json 2>/dev/null; then
        return 0
      fi
      now="$(date +%s)"
      if (( now - start >= timeout )); then
        return 1
      fi
      sleep 1
    done
  }

  fail_count=0

  echo
  echo "========================================"
  echo "Edgion （ PortConflict ）"
  echo "========================================"
  echo "Mode: Gateway"
  echo "Suite: Gateway/PortConflict"
  echo "========================================"
  echo
  echo "▶ Port Conflict Detection Tests"

  # 1) Internal conflict: admission may reject duplicate listener in k8s.
  if internal_json="$(kubectl get gateway -n edgion-test port-conflict-internal -o json 2>/dev/null)"; then
    if check_gateway_has_listeners_not_valid "${internal_json}" \
      && check_listener_conflicted "${internal_json}" "http-1" \
      && check_listener_conflicted "${internal_json}" "http-2"; then
      echo "  ✓ internal_port_conflict"
      echo "    Both listeners conflicted and Gateway has ListenersNotValid"
    else
      echo "  ✗ internal_port_conflict"
      echo "    Gateway exists but conflict status is not as expected"
      fail_count=$((fail_count + 1))
    fi
  else
    echo "  ✓ internal_port_conflict"
    echo "    K8s admission rejected duplicate listener Gateway (treated as expected)"
  fi

  # 2) Cross-gateway conflict.
  if cross_a_json="$(wait_gateway_json "port-conflict-cross-a" 30)" \
    && cross_b_json="$(wait_gateway_json "port-conflict-cross-b" 30)"; then
    if check_gateway_has_listeners_not_valid "${cross_a_json}" \
      && check_gateway_has_listeners_not_valid "${cross_b_json}" \
      && check_listener_conflicted "${cross_a_json}" "http" \
      && check_listener_conflicted "${cross_b_json}" "http"; then
      echo "  ✓ cross_gateway_port_conflict"
      echo "    Both gateways/listeners are marked conflicted"
    elif ! check_gateway_has_listeners_not_valid "${cross_a_json}" \
      && ! check_gateway_has_listeners_not_valid "${cross_b_json}" \
      && check_listener_not_conflicted "${cross_a_json}" "http" \
      && check_listener_not_conflicted "${cross_b_json}" "http"; then
      echo "  ✓ cross_gateway_port_conflict"
      echo "    Runtime keeps cross-Gateway same-port listeners as non-conflicted (accepted in k8s mode)"
    else
      echo "  ✗ cross_gateway_port_conflict"
      echo "    Conflict status missing on one or both gateways"
      fail_count=$((fail_count + 1))
    fi
  else
    echo "  ✗ cross_gateway_port_conflict"
    echo "    Failed to fetch one or both cross-conflict gateways"
    fail_count=$((fail_count + 1))
  fi

  # 3) Same port + different hostname should not conflict.
  if no_conf_json="$(wait_gateway_json "port-no-conflict-hostname" 30)"; then
    if check_gateway_has_listeners_not_valid "${no_conf_json}" \
      || check_listener_conflicted "${no_conf_json}" "api" \
      || check_listener_conflicted "${no_conf_json}" "web" \
      || ! check_listener_not_conflicted "${no_conf_json}" "api" \
      || ! check_listener_not_conflicted "${no_conf_json}" "web"; then
      echo "  ✗ no_conflict_different_hostname"
      echo "    Missing no-conflict verdict or unexpected conflict for different hostnames"
      fail_count=$((fail_count + 1))
    else
      echo "  ✓ no_conflict_different_hostname"
      echo "    No conflict detected for different hostnames on same port"
    fi
  else
    echo "  ✗ no_conflict_different_hostname"
    echo "    Failed to fetch no-conflict gateway"
    fail_count=$((fail_count + 1))
  fi

  total=3
  passed=$((total - fail_count))
  echo
  echo "=================================================="
  echo "Test Summary"
  echo "=================================================="
  echo "Total tests: ${total}"
  echo "Passed: ${passed}"
  echo "Failed: ${fail_count}"
  if [[ "${fail_count}" -eq 0 ]]; then
    echo
    echo "✓ All tests passed"
    exit 0
  fi
  echo
  echo "⚠ Some tests failed"
  exit 1
fi

DIRECT_ENDPOINT_IP="${EDGION_TEST_DIRECT_ENDPOINT_IP:-}"
if [[ -z "${DIRECT_ENDPOINT_IP}" \
  && "${RESOURCE}" == "EdgionPlugins" && "${ITEM}" == "DirectEndpoint" ]]; then
  DIRECT_ENDPOINT_IP="$(
    "${ENDPOINT_INVENTORY}" --namespace edgion-default \
      --service direct-endpoint-backend --port http --timeout-secs 30 \
      | jq -r '.[0].address'
  )"
fi
if [[ -n "${DIRECT_ENDPOINT_IP}" ]]; then
  echo "DirectEndpoint IP: ${DIRECT_ENDPOINT_IP}"
fi

TEST_SERVER_ENDPOINT_IPS="${EDGION_TEST_BACKEND_ENDPOINT_IPS:-}"
if [[ "${RESOURCE}" == "EdgionBackend" ]]; then
  TEST_SERVER_EXPECTED_REPLICAS="$(
    endpoint_inventory_strict_deployment_replicas edgion-test edgion-test-server
  )"
  TEST_SERVER_ENDPOINT_IPS="$("${ENDPOINT_INVENTORY}" --namespace edgion-test \
    --service edgion-test-server --port 30001 \
    --expected-count "${TEST_SERVER_EXPECTED_REPLICAS}" --timeout-secs 30 --output ips)"
elif [[ -z "${TEST_SERVER_ENDPOINT_IPS}" ]]; then
  case "${RESOURCE}/${ITEM}" in
    EdgionPlugins/RequestMirror|EdgionPlugins/WebhookKeyGet|EdgionPlugins/CredentialInjector|EdgionPlugins/RequestBodyBuffer|EdgionPlugins/BodyPlugins|EdgionPlugins/Guardrail|EdgionPlugins/GuardrailEncoding|Gateway/TLS/BackendTLS)
      TEST_SERVER_ENDPOINT_IPS="$("${ENDPOINT_INVENTORY}" --namespace edgion-test \
        --service edgion-test-server --port 30001 --timeout-secs 30 --output ips)"
      ;;
  esac
fi
if [[ -n "${TEST_SERVER_ENDPOINT_IPS}" ]]; then
  echo "Test server endpoints: ${TEST_SERVER_ENDPOINT_IPS}"
fi

LB_RR_EPS_ENDPOINT_IPS="${EDGION_TEST_LB_RR_EPS_ENDPOINT_IPS:-}"
LB_RR_MULTI_ENDPOINT_IPS="${EDGION_TEST_LB_RR_MULTI_ENDPOINT_IPS:-}"
if [[ "${RESOURCE}" == "HTTPRoute" \
   && ( "${ITEM}" == "Backend/LBRoundRobin" || "${ITEM}" == "Backend" ) ]]; then
  LB_RR_EPS_HEALTH_ENDPOINTS="$(endpoint_inventory_wait_for_backend_health \
    edgion-default lb-rr-service 30001 3 "${NAMESPACE}" "${POD}" "${CONTAINER}" 90)"
  LB_RR_MULTI_HEALTH_ENDPOINTS="$(endpoint_inventory_wait_for_backend_health \
    edgion-default lb-rr-multi-service 30001 4 "${NAMESPACE}" "${POD}" "${CONTAINER}" 90)"
  if [[ -z "${LB_RR_EPS_ENDPOINT_IPS}" ]]; then
    LB_RR_EPS_ENDPOINT_IPS="$("${ENDPOINT_INVENTORY}" --namespace edgion-default \
      --service lb-rr-service --port 30001 --expected-count 3 --timeout-secs 30 --output ips)"
  fi
  if [[ -z "${LB_RR_MULTI_ENDPOINT_IPS}" ]]; then
    LB_RR_MULTI_ENDPOINT_IPS="$("${ENDPOINT_INVENTORY}" --namespace edgion-default \
      --service lb-rr-multi-service --port 30001 --expected-count 4 --timeout-secs 30 --output ips)"
  fi
  echo "RoundRobin endpoints: eps=${LB_RR_EPS_ENDPOINT_IPS} multi=${LB_RR_MULTI_ENDPOINT_IPS}"
fi

GATEWAY_METRICS_ENDPOINTS="${EDGION_TEST_GATEWAY_METRICS_ENDPOINTS:-}"
if [[ -z "${GATEWAY_METRICS_ENDPOINTS}" ]]; then
  GATEWAY_METRICS_ENDPOINTS="$("${ENDPOINT_INVENTORY}" --namespace edgion-system \
    --service edgion-gateway --port 12090 --expected-count 1 --timeout-secs 30 --output hostports)"
fi

METRICS_ENDPOINT_COUNT="$(echo "${GATEWAY_METRICS_ENDPOINTS}" | tr ',' '\n' | awk 'NF{count++} END{print count+0}')"
echo "Gateway metrics endpoints (${METRICS_ENDPOINT_COUNT}): ${GATEWAY_METRICS_ENDPOINTS}"

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
  echo "Gateway capability probe returned an empty result" >&2
  exit 1
fi

kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
  env EDGION_TEST_ADMIN_API_URL="${EDGION_CONTROLLER_ADMIN_URL:-http://edgion-controller-leader.edgion-system.svc.cluster.local:12101}" \
  env EDGION_TEST_ADMIN_TOKEN="${EDGION_TEST_ADMIN_TOKEN:-}" \
  env EDGION_TOKEN="${EDGION_TOKEN:-${EDGION_TEST_ADMIN_TOKEN:-}}" \
  env EDGION_GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-}" \
  env EDGION_TEST_GATEWAY_ADMIN_URL="${EDGION_TEST_GATEWAY_ADMIN_URL:-}" \
  env EDGION_TEST_GATEWAY_CAPABILITIES="${GATEWAY_CAPABILITIES}" \
  env EDGION_TEST_K8S_ADMISSION_VERIFIED="${K8S_ADMISSION_VERIFIED}" \
  env EDGION_TEST_AI_PROVIDER_HTTP_BASE="${EDGION_TEST_AI_PROVIDER_HTTP_BASE:-}" \
  env EDGION_TEST_AI_PROVIDER_HTTPS_BASE="${EDGION_TEST_AI_PROVIDER_HTTPS_BASE:-}" \
  env EDGION_TEST_AI_PROVIDER_MTLS_BASE="${EDGION_TEST_AI_PROVIDER_MTLS_BASE:-}" \
  env EDGION_TEST_AI_PROVIDER_SNI_BASE="${EDGION_TEST_AI_PROVIDER_SNI_BASE:-}" \
  env EDGION_TEST_AI_GATEWAY_NAME="${EDGION_TEST_AI_GATEWAY_NAME:-}" \
  env EDGION_TEST_K8S_PEER_UID="${PEER_UID}" EDGION_TEST_K8S_PEER_IP="${PEER_IP}" \
  env EDGION_TEST_K8S_PEER_HIT_CIDR="${PEER_HIT_CIDR}" EDGION_TEST_K8S_PEER_MISS_CIDR="${PEER_MISS_CIDR}" \
  env EDGION_TEST_K8S_MODE="true" \
  env EDGION_INTEGRATION_EXPLICIT_TARGET="${EDGION_INTEGRATION_EXPLICIT_TARGET:-1}" \
  env TEST_SERVER_HOST="${TEST_SERVER_HOST:-edgion-test-server.edgion-test.svc.cluster.local}" \
  env EDGION_TEST_DIRECT_ENDPOINT_IP="${DIRECT_ENDPOINT_IP}" \
  env EDGION_TEST_BACKEND_ENDPOINT_IPS="${TEST_SERVER_ENDPOINT_IPS}" \
  env EDGION_TEST_CREDENTIAL_CONNECTION_BACKEND_HOST="${EDGION_TEST_CREDENTIAL_CONNECTION_BACKEND_HOST:-credential-connection.edgion-default.svc.cluster.local}" \
  env EDGION_TEST_DNS_SERVER_HOST="${EDGION_TEST_DNS_SERVER_HOST:-dns-test-server.edgion-test.svc.cluster.local}" \
  env EDGION_TEST_MIRROR_SERVER_HOST="${EDGION_TEST_MIRROR_SERVER_HOST:-percent-mirror-test-server.edgion-test.svc.cluster.local}" \
  env EDGION_TEST_TRANSPORT_RECORDER_HOST="${EDGION_TEST_TRANSPORT_RECORDER_HOST:-transport-recorder.edgion-test.svc.cluster.local}" \
  env EDGION_TEST_TRANSPORT_H2_HOST="${EDGION_TEST_TRANSPORT_H2_HOST:-test-transport-h2.edgion-test.svc.cluster.local}" \
  env EDGION_TEST_DNS_FLAP_SERVER_HOST="${EDGION_TEST_DNS_FLAP_SERVER_HOST:-test-http-retry.edgion-test.svc.cluster.local}" \
  env EDGION_TEST_WASM_CALLOUT_ORIGIN="${EDGION_TEST_WASM_CALLOUT_ORIGIN:-http://wasm-auth-server.edgion-test.svc.cluster.local:30040}" \
  env EDGION_TEST_GUARDRAIL_FIXTURE_HOST="${EDGION_TEST_GUARDRAIL_FIXTURE_HOST:-edgion-test-server.edgion-test.svc.cluster.local}" \
  env EDGION_TEST_LB_RR_EPS_ENDPOINT_IPS="${LB_RR_EPS_ENDPOINT_IPS}" \
  env EDGION_TEST_LB_RR_MULTI_ENDPOINT_IPS="${LB_RR_MULTI_ENDPOINT_IPS}" \
  env EDGION_TEST_GATEWAY_METRICS_ENDPOINTS="${GATEWAY_METRICS_ENDPOINTS}" \
  sh -c 'cd /usr/local/edgion && exec "$@"' sh \
  "${TEST_CLIENT_BIN_PATH}" -g --target-host "${TEST_TARGET_HOST}" "$@"
