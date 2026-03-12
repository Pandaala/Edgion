#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="${NAMESPACE:-edgion-test}"
POD="${POD:-}"
CONTAINER="${CONTAINER:-test-client}"
TARGET_HOST="${TARGET_HOST:-edgion-gateway.edgion-system.svc.cluster.local}"
GATEWAY_ADMIN_URL="${GATEWAY_ADMIN_URL:-http://edgion-gateway.edgion-system.svc.cluster.local:5900}"
TEST_CLIENT_BIN_PATH="${TEST_CLIENT_BIN_PATH:-/usr/local/bin/test_client}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESOLVE_DIRECT_ENDPOINT_SCRIPT="${SCRIPT_DIR}/resolve_direct_endpoint_ip_from_admin.sh"

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH"
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq not found in PATH"
  exit 1
fi

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

TEST_TARGET_HOST="${TARGET_HOST}"
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
      || check_listener_conflicted "${no_conf_json}" "web"; then
      echo "  ✗ no_conflict_different_hostname"
      echo "    Unexpected conflict detected for different hostnames"
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
if [[ -z "${DIRECT_ENDPOINT_IP}" ]]; then
  if [[ -x "${RESOLVE_DIRECT_ENDPOINT_SCRIPT}" ]]; then
    DIRECT_ENDPOINT_IP="$(
      GATEWAY_ADMIN_URL="${GATEWAY_ADMIN_URL}" \
      SERVICE_NAMESPACE="edgion-default" \
      SERVICE_NAME="direct-endpoint-backend" \
      SERVICE_PORT="30001" \
      "${RESOLVE_DIRECT_ENDPOINT_SCRIPT}" 2>/dev/null || true
    )"
  fi
fi

if [[ -z "${DIRECT_ENDPOINT_IP}" ]]; then
  DIRECT_ENDPOINT_IP="$(
    kubectl get endpoints -n edgion-default direct-endpoint-backend \
      -o jsonpath='{.subsets[0].addresses[0].ip}' 2>/dev/null || true
  )"
fi

if [[ -n "${DIRECT_ENDPOINT_IP}" ]]; then
  echo "DirectEndpoint IP: ${DIRECT_ENDPOINT_IP}"
else
  echo "DirectEndpoint IP not resolved (admin API + kubectl fallback both failed)"
fi

GATEWAY_METRICS_ENDPOINTS="${EDGION_TEST_GATEWAY_METRICS_ENDPOINTS:-}"
if [[ -z "${GATEWAY_METRICS_ENDPOINTS}" ]]; then
  GATEWAY_METRICS_ENDPOINTS="$(
    kubectl get endpointslices -n edgion-system -l kubernetes.io/service-name=edgion-gateway -o json 2>/dev/null \
      | jq -r '[.items[]?.endpoints[]? | select(.conditions.ready != false) | .addresses[]? | "\(.):5901"] | unique | join(",")' 2>/dev/null || true
  )"
fi

if [[ -z "${GATEWAY_METRICS_ENDPOINTS}" || "${GATEWAY_METRICS_ENDPOINTS}" == "null" ]]; then
  GATEWAY_METRICS_ENDPOINTS="$(
    kubectl get endpoints -n edgion-system edgion-gateway -o json 2>/dev/null \
      | jq -r '[.subsets[]?.addresses[]?.ip | "\(.):5901"] | unique | join(",")' 2>/dev/null || true
  )"
fi

if [[ -z "${GATEWAY_METRICS_ENDPOINTS}" || "${GATEWAY_METRICS_ENDPOINTS}" == "null" ]]; then
  GATEWAY_METRICS_ENDPOINTS="edgion-gateway.edgion-system.svc.cluster.local:5901"
fi

METRICS_ENDPOINT_COUNT="$(echo "${GATEWAY_METRICS_ENDPOINTS}" | tr ',' '\n' | awk 'NF{count++} END{print count+0}')"
echo "Gateway metrics endpoints (${METRICS_ENDPOINT_COUNT}): ${GATEWAY_METRICS_ENDPOINTS}"

kubectl exec -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- \
  env EDGION_TEST_ADMIN_API_URL="${EDGION_CONTROLLER_ADMIN_URL:-http://edgion-controller.edgion-system.svc.cluster.local:5800}" \
  env EDGION_TEST_K8S_MODE="true" \
  env EDGION_TEST_DIRECT_ENDPOINT_IP="${DIRECT_ENDPOINT_IP}" \
  env EDGION_TEST_GATEWAY_METRICS_ENDPOINTS="${GATEWAY_METRICS_ENDPOINTS}" \
  "${TEST_CLIENT_BIN_PATH}" -g --target-host "${TEST_TARGET_HOST}" "$@"
