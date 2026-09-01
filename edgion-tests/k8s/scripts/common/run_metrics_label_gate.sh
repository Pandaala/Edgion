#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENDPOINT_INVENTORY="${SCRIPT_DIR}/endpoint_inventory.sh"

GATEWAY_NAMESPACE="${GATEWAY_NAMESPACE:-edgion-system}"
GATEWAY_SERVICE="${GATEWAY_SERVICE:-edgion-gateway}"
GATEWAY_METRICS_PORT="${GATEWAY_METRICS_PORT:-metrics-12090}"
TEST_CLIENT_NAMESPACE="${TEST_CLIENT_NAMESPACE:-edgion-test}"
TEST_CLIENT_SELECTOR="${TEST_CLIENT_SELECTOR:-app=edgion-test-client}"
TEST_CLIENT_CONTAINER="${TEST_CLIENT_CONTAINER:-test-client}"
TEST_CLIENT_POD="${TEST_CLIENT_POD:-}"
INVENTORY_TIMEOUT_SECS="${INVENTORY_TIMEOUT_SECS:-30}"
PROBE_TIMEOUT_SECS="${PROBE_TIMEOUT_SECS:-5}"

usage() {
  cat <<'EOF'
Usage: run_metrics_label_gate.sh [options]

Run the canonical MetricsLabelWhitelist final gate against every live Gateway
metrics EndpointSlice address from an in-cluster test-client Pod.

Options:
  --gateway-namespace NAMESPACE  Gateway Service namespace (default: edgion-system)
  --gateway-service SERVICE      Gateway Service name (default: edgion-gateway)
  --metrics-port PORT            EndpointSlice port name or number (default: metrics-12090)
  --test-client-namespace NS     Test-client namespace (default: edgion-test)
  --test-client-selector LABELS  Test-client Pod selector (default: app=edgion-test-client)
  --test-client-container NAME   Test-client container (default: test-client)
  --test-client-pod POD          Use this running test-client Pod instead of discovering one
  --inventory-timeout-secs N     EndpointSlice wait deadline (default: 30)
  --probe-timeout-secs N         Per-endpoint curl deadline (default: 5)
  -h, --help                     Show this help

Runtime stdout protocol:
  @@FINAL_GATE_RESULT MetricsLabelWhitelist PASS|FAIL endpoints=N checked=N
  @@FINAL_GATE_SUMMARY total=1 failed=0|1
EOF
}

error() {
  echo "ERROR: $*" >&2
}

validate_uint() {
  local value="$1"
  local label="$2"
  case "$value" in
    ""|*[!0-9]*)
      error "$label must be a non-negative integer"
      return 1
      ;;
  esac
}

emit_result() {
  local status="$1"
  local endpoint_count="$2"
  local checked_count="$3"
  local failed=1
  if [ "$status" = "PASS" ]; then
    failed=0
  fi
  printf '@@FINAL_GATE_RESULT MetricsLabelWhitelist %s endpoints=%s checked=%s\n' \
    "$status" "$endpoint_count" "$checked_count"
  printf '@@FINAL_GATE_SUMMARY total=1 failed=%s\n' "$failed"
}

runtime_failure() {
  local endpoint_count="$1"
  local checked_count="$2"
  emit_result FAIL "$endpoint_count" "$checked_count"
  return 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --gateway-namespace)
      [ "$#" -ge 2 ] || { error "--gateway-namespace requires a value"; exit 2; }
      GATEWAY_NAMESPACE="$2"
      shift 2
      ;;
    --gateway-service)
      [ "$#" -ge 2 ] || { error "--gateway-service requires a value"; exit 2; }
      GATEWAY_SERVICE="$2"
      shift 2
      ;;
    --metrics-port)
      [ "$#" -ge 2 ] || { error "--metrics-port requires a value"; exit 2; }
      GATEWAY_METRICS_PORT="$2"
      shift 2
      ;;
    --test-client-namespace)
      [ "$#" -ge 2 ] || { error "--test-client-namespace requires a value"; exit 2; }
      TEST_CLIENT_NAMESPACE="$2"
      shift 2
      ;;
    --test-client-selector)
      [ "$#" -ge 2 ] || { error "--test-client-selector requires a value"; exit 2; }
      TEST_CLIENT_SELECTOR="$2"
      shift 2
      ;;
    --test-client-container)
      [ "$#" -ge 2 ] || { error "--test-client-container requires a value"; exit 2; }
      TEST_CLIENT_CONTAINER="$2"
      shift 2
      ;;
    --test-client-pod)
      [ "$#" -ge 2 ] || { error "--test-client-pod requires a value"; exit 2; }
      TEST_CLIENT_POD="$2"
      shift 2
      ;;
    --inventory-timeout-secs)
      [ "$#" -ge 2 ] || { error "--inventory-timeout-secs requires a value"; exit 2; }
      INVENTORY_TIMEOUT_SECS="$2"
      shift 2
      ;;
    --probe-timeout-secs)
      [ "$#" -ge 2 ] || { error "--probe-timeout-secs requires a value"; exit 2; }
      PROBE_TIMEOUT_SECS="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      error "unknown option: $1"
      usage >&2
      exit 2
      ;;
  esac
done

validate_uint "$INVENTORY_TIMEOUT_SECS" "--inventory-timeout-secs" || exit 2
validate_uint "$PROBE_TIMEOUT_SECS" "--probe-timeout-secs" || exit 2
if [ "$PROBE_TIMEOUT_SECS" -eq 0 ]; then
  error "--probe-timeout-secs must be greater than zero"
  exit 2
fi
if ! command -v kubectl >/dev/null 2>&1; then
  error "kubectl not found in PATH"
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  error "jq not found in PATH"
  exit 2
fi
if [ ! -x "$ENDPOINT_INVENTORY" ]; then
  error "EndpointSlice inventory helper is not executable: $ENDPOINT_INVENTORY"
  exit 2
fi

endpoint_json=""
if ! endpoint_json=$("$ENDPOINT_INVENTORY" \
  --namespace "$GATEWAY_NAMESPACE" \
  --service "$GATEWAY_SERVICE" \
  --port "$GATEWAY_METRICS_PORT" \
  --timeout-secs "$INVENTORY_TIMEOUT_SECS" \
  --output json); then
  error "Gateway metrics EndpointSlice inventory failed"
  runtime_failure 0 0
  exit 1
fi

if ! endpoint_count=$(printf '%s\n' "$endpoint_json" | jq -er 'if type == "array" then length else error("not an array") end'); then
  error "Gateway metrics EndpointSlice inventory returned invalid JSON"
  runtime_failure 0 0
  exit 1
fi
if [ "$endpoint_count" -eq 0 ]; then
  error "Gateway metrics EndpointSlice inventory returned no live endpoints"
  runtime_failure 0 0
  exit 1
fi

if [ -z "$TEST_CLIENT_POD" ]; then
  if ! TEST_CLIENT_POD=$(kubectl get pods \
    --namespace "$TEST_CLIENT_NAMESPACE" \
    --selector "$TEST_CLIENT_SELECTOR" \
    --field-selector status.phase=Running \
    --sort-by=.metadata.creationTimestamp \
    --output custom-columns=NAME:.metadata.name,DELETING:.metadata.deletionTimestamp \
    --no-headers 2>/dev/null \
    | awk '$2 == "<none>" {print $1}' \
    | tail -n 1); then
    error "Failed to discover a running test-client Pod"
    runtime_failure "$endpoint_count" 0
    exit 1
  fi
fi
if [ -z "$TEST_CLIENT_POD" ]; then
  error "No running, non-terminating test-client Pod found"
  runtime_failure "$endpoint_count" 0
  exit 1
fi

checked_count=0
failed=false
while IFS= read -r endpoint; do
  address=$(printf '%s\n' "$endpoint" | jq -er '.address')
  port=$(printf '%s\n' "$endpoint" | jq -er '.port')
  address_type=$(printf '%s\n' "$endpoint" | jq -er '.addressType')
  if [ "$address_type" = "IPv6" ]; then
    authority="[${address}]:${port}"
  else
    authority="${address}:${port}"
  fi
  url="http://${authority}/metrics"
  checked_count=$((checked_count + 1))

  metrics_body=""
  if ! metrics_body=$(kubectl exec \
    --namespace "$TEST_CLIENT_NAMESPACE" \
    --container "$TEST_CLIENT_CONTAINER" \
    "$TEST_CLIENT_POD" -- \
    curl --fail --silent --show-error --max-time "$PROBE_TIMEOUT_SECS" "$url"); then
    error "Metrics endpoint could not be read from test-client Pod: $url"
    failed=true
    continue
  fi
  if [ -z "$metrics_body" ]; then
    error "Metrics endpoint returned an empty body: $url"
    failed=true
    continue
  fi

  roles=$(printf '%s\n' "$metrics_body" | grep -oE 'role="[^"]+"' | sort -u || true)
  while IFS= read -r role; do
    [ -n "$role" ] || continue
    if [ "$role" != 'role="controller"' ]; then
      error "Metrics endpoint has unexpected role label: $url: $role (allowed: controller)"
      failed=true
    fi
  done <<EOF
$roles
EOF
done < <(printf '%s\n' "$endpoint_json" | jq -c '.[]')

if [ "$checked_count" -ne "$endpoint_count" ]; then
  error "Endpoint accounting mismatch: discovered=$endpoint_count checked=$checked_count"
  failed=true
fi
if [ "$failed" = "true" ]; then
  runtime_failure "$endpoint_count" "$checked_count"
  exit 1
fi

emit_result PASS "$endpoint_count" "$checked_count"
