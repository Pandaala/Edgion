#!/usr/bin/env bash
# =============================================================================
# list-keys-fs.sh
#
# Verifies FS-backend behavior for the new metadata-only keys endpoints
# (Card 02 of list-keys-api, replacing Card 01's not-supported placeholder):
#   - GET /api/v1/keys/cluster/GatewayClass         -> 200 + array response
#   - GET /api/v1/keys/namespaced/HTTPRoute         -> 200 + metadata-only list
#   - data items have no `spec` and no `status` keys
#   - continue_token is null (FS backend does not paginate)
#   - Regression: legacy /api/v1/namespaced/HTTPRoute still returns full
#     content (data items carry `spec`)
#
# Spins up its own controller in FileSystem mode; no external dependencies
# beyond a built `edgion-controller` binary and `jq`.
# =============================================================================
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC}   $1"; }
log_error()   { echo -e "${RED}[ERR]${NC}  $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() { echo; echo -e "${BLUE}=== $1 ===${NC}"; }

command -v jq >/dev/null      || { log_warn "jq not installed; SKIP"; exit 0; }
command -v python3 >/dev/null || { log_warn "python3 not installed; SKIP"; exit 0; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"

if [[ ! -x "$CONTROLLER_BIN" ]]; then
    log_warn "edgion-controller binary not found at $CONTROLLER_BIN; SKIP"
    log_info "Build with: cargo build --bin edgion-controller"
    exit 0
fi

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/list_keys_fs_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
CONF_WATCH_DIR="${WORK_DIR}/conf"
LOG_DIR="${WORK_DIR}/logs"

CONTROLLER_ADMIN_PORT=15824
CONTROLLER_GRPC_PORT=15825
# /health and /ready live on the dedicated probe listener, not the admin API.
# Pin it (and the metrics listener) to test-specific ports so they do not fall
# back to the shared defaults (12100 / 12190) and collide with the main stack.
CONTROLLER_PROBE_PORT=15828
CONTROLLER_METRICS_PORT=15829
ADMIN_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
PROBE_URL="http://127.0.0.1:${CONTROLLER_PROBE_PORT}"

CONTROLLER_PID=""
TEST_PASSED=0

cleanup() {
    log_section "Cleanup"
    if [[ -n "$CONTROLLER_PID" ]] && kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        kill "$CONTROLLER_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$CONTROLLER_PID" 2>/dev/null || true
    fi
    if [[ "$TEST_PASSED" -eq 1 ]]; then
        rm -rf "$WORK_DIR"
    else
        log_warn "Keeping WORK_DIR for post-mortem: $WORK_DIR"
    fi
}
trap cleanup EXIT

log_section "Setup"
mkdir -p "$CONFIG_DIR" "$CONF_WATCH_DIR" "$LOG_DIR"
ln -s "${PROJECT_ROOT}/config/crd" "${CONFIG_DIR}/crd"

cat > "${CONFIG_DIR}/edgion-controller.yaml" <<EOF
work_dir: "${WORK_DIR}"

server:
  grpc_listen: "127.0.0.1:${CONTROLLER_GRPC_PORT}"
  admin_listen: "127.0.0.1:${CONTROLLER_ADMIN_PORT}"
  probe_listen: "127.0.0.1:${CONTROLLER_PROBE_PORT}"
  metrics_listen: "127.0.0.1:${CONTROLLER_METRICS_PORT}"

logging:
  log_dir: "${LOG_DIR}"
  log_prefix: edgion-controller
  log_level: info
  console: false

conf_center:
  type: file_system
  conf_dir: "${CONF_WATCH_DIR}"

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
EOF

NAMESPACE="ns-list-keys"
FIXTURE_COUNT=25
mkdir -p "${CONF_WATCH_DIR}/${NAMESPACE}"
for i in $(seq 1 ${FIXTURE_COUNT}); do
  cat > "${CONF_WATCH_DIR}/${NAMESPACE}/HTTPRoute_${NAMESPACE}_route-${i}.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route-${i}
  namespace: ${NAMESPACE}
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /r${i}
EOF
done
log_info "Config + ${FIXTURE_COUNT} HTTPRoute fixtures written"

log_section "Start Controller (FileSystem mode)"
# Admin credentials for the Controller we start below. It no longer
# auto-generates them (see the credential-autogen-removal change); honor inherited
# EDGION_ADMIN_* (e.g. from run_integration.sh) or fall back to fixed test values.
# The Controller synthesizes a local_auth provider from these. JWT secret >= 32 bytes.
export EDGION_ADMIN_USERNAME="${EDGION_ADMIN_USERNAME:-admin}"
export EDGION_ADMIN_PASSWORD="${EDGION_ADMIN_PASSWORD:-edgion-integration-test-admin-pw}"
export EDGION_ADMIN_JWT_SECRET="${EDGION_ADMIN_JWT_SECRET:-edgion-integration-test-jwt-secret-0123456789}"
"$CONTROLLER_BIN" --config-file "${CONFIG_DIR}/edgion-controller.yaml" \
    > "${LOG_DIR}/controller.stdout.log" 2> "${LOG_DIR}/controller.stderr.log" &
CONTROLLER_PID=$!
log_info "Controller PID=$CONTROLLER_PID"

for i in $(seq 1 30); do
    if curl -sf "${PROBE_URL}/health" > /dev/null 2>&1; then
        log_success "Controller ready"
        break
    fi
    if ! kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_error "Controller exited during startup"
        tail -40 "${LOG_DIR}/controller.stderr.log" || true
        exit 1
    fi
    sleep 1
done

log_section "Authenticate"
# Log in with the EDGION_ADMIN_* credentials exported above. Retry to absorb the
# brief window where /health is up but the auth subsystem is still initializing.
ADMIN_TOKEN=""
for i in $(seq 1 15); do
    ADMIN_TOKEN=$(curl -s -H "Content-Type: application/json" \
        -d "{\"username\":\"${EDGION_ADMIN_USERNAME}\",\"password\":\"${EDGION_ADMIN_PASSWORD}\"}" \
        "${ADMIN_URL}/api/v1/auth/login" \
        | python3 -c "import sys,json; print(json.load(sys.stdin).get('data',{}).get('token',''))" 2>/dev/null || true)
    [[ -n "$ADMIN_TOKEN" ]] && break
    sleep 1
done
if [[ -z "$ADMIN_TOKEN" ]]; then
    log_error "Login failed (admin auth not ready)"
    exit 1
fi
log_success "Got admin token"

curl_admin() { curl -s -H "Authorization: Bearer ${ADMIN_TOKEN}" "$@"; }

# Allow informer to finish initial reconcile
sleep 2

log_section "Assert /api/v1/keys/namespaced/HTTPRoute returns metadata-only list"
RESPONSE=$(curl_admin "${ADMIN_URL}/api/v1/keys/namespaced/HTTPRoute")

# Exact fixture count — proves no entry got silently dropped by projection
echo "$RESPONSE" | jq -e ".data | length == ${FIXTURE_COUNT}" >/dev/null \
    || { log_error "Expected ${FIXTURE_COUNT} items, got: $(echo "$RESPONSE" | jq -c '.data | length')"; echo "$RESPONSE" | jq .; exit 1; }

# Structural projection check: each item must have ONLY the ResourceKey-shaped
# keys (kind / apiVersion / metadata). `.spec == null` alone would be vacuous
# because ResourceKey is a typed struct without that field, so we verify the
# key-set positively.
echo "$RESPONSE" | jq -e '.data | all((keys | sort) == ["apiVersion","kind","metadata"])' >/dev/null \
    || { log_error "Item key-set is not exactly {apiVersion, kind, metadata}; projection let extra fields through"; echo "$RESPONSE" | jq '.data[0]'; exit 1; }
echo "$RESPONSE" | jq -e '.data | all(.metadata.name != null and .metadata.name != "")' >/dev/null \
    || { log_error "Some item is missing metadata.name"; exit 1; }
echo "$RESPONSE" | jq -e '.continue_token == null or .continue_token == ""' >/dev/null \
    || { log_error "FS backend should not paginate — continue_token must be null"; exit 1; }
log_success "namespaced keys endpoint: ${FIXTURE_COUNT} items, ResourceKey-shaped, no pagination"

log_section "Assert /api/v1/keys/cluster/GatewayClass returns 200"
STATUS=$(curl_admin -o /tmp/list-keys-fs-cluster.json -w "%{http_code}" \
    "${ADMIN_URL}/api/v1/keys/cluster/GatewayClass")
if [[ "$STATUS" != "200" ]]; then
    log_error "Expected 200, got $STATUS"
    cat /tmp/list-keys-fs-cluster.json
    exit 1
fi
jq -e '.data | type == "array"' /tmp/list-keys-fs-cluster.json >/dev/null \
    || { log_error "cluster response data is not an array"; cat /tmp/list-keys-fs-cluster.json; exit 1; }
log_success "cluster keys endpoint: 200 + array response"

log_section "Regression: legacy /api/v1/namespaced/HTTPRoute returns full content with spec"
LEGACY=$(curl_admin "${ADMIN_URL}/api/v1/namespaced/HTTPRoute")
echo "$LEGACY" | jq -e ".data | length == ${FIXTURE_COUNT}" >/dev/null \
    || { log_error "Legacy path lost items, got: $(echo "$LEGACY" | jq -c '.data | length')"; exit 1; }
echo "$LEGACY" | jq -e '.data | any(.spec != null)' >/dev/null \
    || { log_error "Legacy path dropped spec — regression!"; exit 1; }
log_success "Legacy namespaced path preserved"

TEST_PASSED=1
log_section "PASS: list-keys-fs.sh"
