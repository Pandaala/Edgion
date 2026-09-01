#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENDPOINT_INVENTORY="${SCRIPT_DIR}/endpoint_inventory.sh"

TEST_CLIENT_NAMESPACE="${TEST_CLIENT_NAMESPACE:-edgion-test}"
TEST_CLIENT_SELECTOR="${TEST_CLIENT_SELECTOR:-app=edgion-test-client}"
TEST_CLIENT_CONTAINER="${TEST_CLIENT_CONTAINER:-test-client}"
TEST_CLIENT_POD="${TEST_CLIENT_POD:-}"
CACHE_RESIDUE_BIN="${CACHE_RESIDUE_BIN:-/usr/local/bin/cache_residue}"
CONTROLLER_NAMESPACE="${CONTROLLER_NAMESPACE:-edgion-system}"
CONTROLLER_SERVICE="${CONTROLLER_SERVICE:-edgion-controller}"
CONTROLLER_ADMIN_PORT="${CONTROLLER_ADMIN_PORT:-http}"
CONTROLLER_ADDR="${CONTROLLER_ADDR:-}"
CONTROLLER_ADMIN_TOKEN="${EDGION_TEST_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
GATEWAY_NAMESPACE="${GATEWAY_NAMESPACE:-edgion-system}"
GATEWAY_SERVICE="${GATEWAY_SERVICE:-edgion-gateway}"
GATEWAY_ADMIN_PORT="${GATEWAY_ADMIN_PORT:-admin-12001}"
GATEWAY_ADDR="${GATEWAY_ADDR:-}"
GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-integration-gateway-admin-token-0123456789}"
INVENTORY_TIMEOUT_SECS="${INVENTORY_TIMEOUT_SECS:-30}"
VALIDATOR_TIMEOUT_SECS="${VALIDATOR_TIMEOUT_SECS:-30}"
ROOT_CA_DESCRIPTION='Contains a CA bundle that can be used to verify the kube-apiserver when using internal endpoints such as the internal service IP or kubernetes.default.svc. No other usage is guaranteed across distributions of Kubernetes clusters.'

usage() {
  cat <<'EOF'
Usage: run_cache_residue_gate.sh [options]

Run cache_residue inside the K8s test-client Pod against the Controller and
Gateway Pod Admin APIs discovered through EndpointSlices. The validator always
runs with --mode fail. Explicit address options override discovery.

Options:
  --test-client-pod POD       Use this test-client Pod instead of discovering one
  --cache-residue-bin PATH    In-Pod validator path (default: /usr/local/bin/cache_residue)
  --controller-addr URL       Override the discovered Controller Pod Admin URL
  --controller-token TOKEN    Controller test admin bearer token
  --gateway-addr URL          Override the discovered Gateway Pod Admin URL
  --gateway-token TOKEN       Gateway test admin bearer token
  --inventory-timeout-secs N  EndpointSlice wait deadline (default: 30)
  --validator-timeout-secs N  cache_residue poll deadline (default: 30)
  -h, --help                  Show this help

Runtime stdout protocol:
  @@FINAL_GATE_RESULT CacheResidue PASS|FAIL validator_rc=N|not-run reason=REASON
  @@FINAL_GATE_SUMMARY total=1 failed=0|1
EOF
}

error() {
  echo "ERROR: $*" >&2
}

emit_result() {
  local status="$1"
  local validator_rc="$2"
  local reason="$3"
  local failed=1
  if [ "$status" = "PASS" ]; then
    failed=0
  fi
  printf '@@FINAL_GATE_RESULT CacheResidue %s validator_rc=%s reason=%s\n' \
    "$status" "$validator_rc" "$reason"
  printf '@@FINAL_GATE_SUMMARY total=1 failed=%s\n' "$failed"
}

validate_positive_uint() {
  local value="$1"
  local label="$2"
  case "$value" in
    ""|*[!0-9]*)
      error "$label must be a positive integer"
      return 1
      ;;
  esac
  if [ "$value" -eq 0 ]; then
    error "$label must be a positive integer"
    return 1
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --test-client-pod)
      [ "$#" -ge 2 ] || { error "--test-client-pod requires a value"; exit 2; }
      TEST_CLIENT_POD="$2"
      shift 2
      ;;
    --cache-residue-bin)
      [ "$#" -ge 2 ] || { error "--cache-residue-bin requires a value"; exit 2; }
      CACHE_RESIDUE_BIN="$2"
      shift 2
      ;;
    --controller-addr)
      [ "$#" -ge 2 ] || { error "--controller-addr requires a value"; exit 2; }
      CONTROLLER_ADDR="$2"
      shift 2
      ;;
    --controller-token)
      [ "$#" -ge 2 ] || { error "--controller-token requires a value"; exit 2; }
      CONTROLLER_ADMIN_TOKEN="$2"
      shift 2
      ;;
    --gateway-addr)
      [ "$#" -ge 2 ] || { error "--gateway-addr requires a value"; exit 2; }
      GATEWAY_ADDR="$2"
      shift 2
      ;;
    --gateway-token)
      [ "$#" -ge 2 ] || { error "--gateway-token requires a value"; exit 2; }
      GATEWAY_ADMIN_TOKEN="$2"
      shift 2
      ;;
    --inventory-timeout-secs)
      [ "$#" -ge 2 ] || { error "--inventory-timeout-secs requires a value"; exit 2; }
      INVENTORY_TIMEOUT_SECS="$2"
      shift 2
      ;;
    --validator-timeout-secs)
      [ "$#" -ge 2 ] || { error "--validator-timeout-secs requires a value"; exit 2; }
      VALIDATOR_TIMEOUT_SECS="$2"
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

validate_positive_uint "$INVENTORY_TIMEOUT_SECS" "--inventory-timeout-secs" || exit 2
validate_positive_uint "$VALIDATOR_TIMEOUT_SECS" "--validator-timeout-secs" || exit 2
if [ -z "$CONTROLLER_ADMIN_TOKEN" ] || [ -z "$GATEWAY_ADMIN_TOKEN" ]; then
  error "Controller and Gateway admin tokens are required"
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

# Every namespace receives a Kubernetes-owned kube-root-ca.crt ConfigMap. The
# API server recreates these platform objects after wipe-all, so discover their
# exact identities before the destructive gate and pass only verified baseline
# objects to the validator. A same-named ConfigMap with a different annotation,
# additional data, or binary data remains visible as residue.
if ! ROOT_CA_CONFIGMAP_KEYS="$(
  kubectl get configmaps --all-namespaces --output json \
    | jq -r --arg description "$ROOT_CA_DESCRIPTION" '
        .items[]
        | select(.metadata.name == "kube-root-ca.crt")
        | select(.metadata.namespace != null and .metadata.namespace != "")
        | select(.metadata.annotations["kubernetes.io/description"] == $description)
        | select((.data | type) == "object" and (.data | keys) == ["ca.crt"])
        | select(((.binaryData // {}) | length) == 0)
        | "\(.metadata.namespace)/\(.metadata.name)"
      '
)"; then
  error "Failed to inventory Kubernetes-owned root CA ConfigMaps"
  emit_result FAIL not-run root-ca-inventory
  exit 1
fi

IGNORE_CONTROLLER_CONFIGMAP_ARGS=()
root_ca_configmap_count=0
while IFS= read -r configmap_key; do
  [ -n "$configmap_key" ] || continue
  IGNORE_CONTROLLER_CONFIGMAP_ARGS+=(--ignore-controller-configmap "$configmap_key")
  root_ca_configmap_count=$((root_ca_configmap_count + 1))
done <<< "$ROOT_CA_CONFIGMAP_KEYS"
echo "Verified Kubernetes-owned root CA ConfigMaps: ${root_ca_configmap_count}" >&2

# The live Controller intentionally owns and recreates these two conf-sync CA
# Secrets after wipe-all. Admit only the two reserved identities with their
# exact ownership, type, and key shapes. The Rust validator repeats this
# profile check on every post-wipe poll before subtracting them from residue.
if ! CONF_SYNC_SECRET_KEYS="$(
  kubectl get secrets --namespace "$CONTROLLER_NAMESPACE" --output json \
    | jq -r '
        .items[]
        | select(.metadata.namespace != null and .metadata.namespace != "")
        | select(.metadata.name == "edgion-conf-sync-ca" or .metadata.name == "edgion-conf-sync-certs")
        | select(.metadata.labels["edgion.io/managed-by"] == "conf-sync-ca")
        | select(.type == "Opaque")
        | select((.data | type) == "object")
        | select(all(.data[]; type == "string" and length > 0))
        | select(
            if .metadata.name == "edgion-conf-sync-ca" then
              (.data | keys) == ["ca.crt", "ca.key"]
            else
              (.data | keys) == ["ca.crt", "client.crt", "client.key"]
            end
          )
        | "\(.metadata.namespace)/\(.metadata.name)"
      ' \
    | sort
)"; then
  error "Failed to inventory Controller-owned conf-sync Secrets"
  emit_result FAIL not-run conf-sync-secret-inventory
  exit 1
fi
EXPECTED_CONF_SYNC_SECRET_KEYS="${CONTROLLER_NAMESPACE}/edgion-conf-sync-ca
${CONTROLLER_NAMESPACE}/edgion-conf-sync-certs"
if [ "$CONF_SYNC_SECRET_KEYS" != "$EXPECTED_CONF_SYNC_SECRET_KEYS" ]; then
  error "Controller-owned conf-sync Secret inventory is incomplete or invalid"
  emit_result FAIL not-run conf-sync-secret-inventory
  exit 1
fi

IGNORE_CONTROLLER_CONF_SYNC_SECRET_ARGS=()
while IFS= read -r secret_key; do
  [ -n "$secret_key" ] || continue
  IGNORE_CONTROLLER_CONF_SYNC_SECRET_ARGS+=(--ignore-controller-conf-sync-secret "$secret_key")
done <<< "$CONF_SYNC_SECRET_KEYS"
echo "Verified Controller-owned conf-sync Secrets: 2" >&2

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
    emit_result FAIL not-run missing-pod
    exit 1
  fi
fi
if [ -z "$TEST_CLIENT_POD" ]; then
  error "No running, non-terminating test-client Pod found"
  emit_result FAIL not-run missing-pod
  exit 1
fi

if ! kubectl exec \
  --namespace "$TEST_CLIENT_NAMESPACE" \
  --container "$TEST_CLIENT_CONTAINER" \
  "$TEST_CLIENT_POD" -- \
  test -x "$CACHE_RESIDUE_BIN"; then
  error "cache_residue binary is missing or not executable in test-client Pod: $CACHE_RESIDUE_BIN"
  emit_result FAIL not-run missing-binary
  exit 1
fi

if ! controller_hostport=$("$ENDPOINT_INVENTORY" \
  --namespace "$CONTROLLER_NAMESPACE" \
  --service "$CONTROLLER_SERVICE" \
  --port "$CONTROLLER_ADMIN_PORT" \
  --expected-count 1 \
  --timeout-secs "$INVENTORY_TIMEOUT_SECS" \
  --output hostports); then
  error "Controller Admin Service has no live EndpointSlice address"
  emit_result FAIL not-run missing-controller-endpoints
  exit 1
fi
if ! gateway_hostport=$("$ENDPOINT_INVENTORY" \
  --namespace "$GATEWAY_NAMESPACE" \
  --service "$GATEWAY_SERVICE" \
  --port "$GATEWAY_ADMIN_PORT" \
  --expected-count 1 \
  --timeout-secs "$INVENTORY_TIMEOUT_SECS" \
  --output hostports); then
  error "Gateway Admin Service has no live EndpointSlice address"
  emit_result FAIL not-run missing-gateway-endpoints
  exit 1
fi

# cache_residue deletes every configuration object, including the Admin
# Services themselves. Keep talking directly to the already-running Pods after
# that wipe instead of depending on Service DNS that intentionally disappears.
if [ -z "$CONTROLLER_ADDR" ]; then
  CONTROLLER_ADDR="http://${controller_hostport}"
fi
if [ -z "$GATEWAY_ADDR" ]; then
  GATEWAY_ADDR="http://${gateway_hostport}"
fi

VALIDATOR_OUTPUT="$(mktemp /tmp/edgion-cache-residue-output.XXXXXX)"
cleanup() {
  rm -f "$VALIDATOR_OUTPUT"
}
trap cleanup EXIT INT TERM

set +e
kubectl exec \
  --namespace "$TEST_CLIENT_NAMESPACE" \
  --container "$TEST_CLIENT_CONTAINER" \
  "$TEST_CLIENT_POD" -- \
  "$CACHE_RESIDUE_BIN" \
  --controller-addr "$CONTROLLER_ADDR" \
  --gateway-addr "$GATEWAY_ADDR" \
  --controller-token "$CONTROLLER_ADMIN_TOKEN" \
  --gateway-token "$GATEWAY_ADMIN_TOKEN" \
  --timeout-secs "$VALIDATOR_TIMEOUT_SECS" \
  "${IGNORE_CONTROLLER_CONFIGMAP_ARGS[@]}" \
  "${IGNORE_CONTROLLER_CONF_SYNC_SECRET_ARGS[@]}" \
  --mode fail > "$VALIDATOR_OUTPUT" 2>&1
validator_rc=$?
set -e

if [ ! -s "$VALIDATOR_OUTPUT" ]; then
  error "cache_residue produced no output"
  emit_result FAIL "$validator_rc" empty-output
  exit 1
fi

case "$validator_rc" in
  0)
    emit_result PASS 0 clean
    ;;
  1)
    error "cache_residue detected residue"
    sed 's/^/  /' "$VALIDATOR_OUTPUT" >&2
    emit_result FAIL 1 residue
    exit 1
    ;;
  2)
    error "cache_residue reported an infrastructure error"
    sed 's/^/  /' "$VALIDATOR_OUTPUT" >&2
    emit_result FAIL 2 infrastructure
    exit 1
    ;;
  *)
    error "cache_residue returned an unexpected exit code: $validator_rc"
    sed 's/^/  /' "$VALIDATOR_OUTPUT" >&2
    emit_result FAIL "$validator_rc" unexpected-rc
    exit 1
    ;;
esac
