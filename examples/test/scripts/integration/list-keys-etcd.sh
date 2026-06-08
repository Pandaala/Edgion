#!/usr/bin/env bash
# =============================================================================
# list-keys-etcd.sh
#
# Verifies Etcd-backend behavior for the new metadata-only keys endpoints
# (Card 02 of list-keys-api):
#   - GET /api/v1/keys/cluster/GatewayClass         -> 200 + array response
#   - GET /api/v1/keys/namespaced/HTTPRoute         -> 200 + metadata-only items
#   - data items have no `spec` and no `status` keys (only metadata)
#   - continue_token is null (Etcd does not paginate via the keys path in
#     Card 02; pagination is a future enhancement to get_list_by_kind)
#   - Regression: legacy /api/v1/namespaced/HTTPRoute still returns full
#     content (data items carry `spec`)
#
# Requires a reachable etcd. Skips if ETCD_ENDPOINT is not exported.
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

if [[ -z "${ETCD_ENDPOINT:-}" ]]; then
    log_warn "ETCD_ENDPOINT not set; SKIP"
    log_info "Run with: ETCD_ENDPOINT=http://127.0.0.1:2379 $0"
    exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"

if [[ ! -x "$CONTROLLER_BIN" ]]; then
    log_warn "edgion-controller binary not found at $CONTROLLER_BIN; SKIP"
    exit 0
fi

if ! curl -sf --max-time 3 "${ETCD_ENDPOINT}/health" > /dev/null 2>&1; then
    log_warn "Cannot reach etcd at $ETCD_ENDPOINT; SKIP"
    exit 0
fi

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/list_keys_etcd_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
LOG_DIR="${WORK_DIR}/logs"

CONTROLLER_ADMIN_PORT=15826
CONTROLLER_GRPC_PORT=15827
# /health and /ready live on the dedicated probe listener, not the admin API.
# Pin it (and the metrics listener) to test-specific ports so they do not fall
# back to the shared defaults (12100 / 12190) and collide with the main stack.
CONTROLLER_PROBE_PORT=15822
CONTROLLER_METRICS_PORT=15823
ADMIN_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
PROBE_URL="http://127.0.0.1:${CONTROLLER_PROBE_PORT}"
ETCD_PREFIX="/edgion-list-keys-test-${TIMESTAMP}"

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
mkdir -p "$CONFIG_DIR" "$LOG_DIR"
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
  type: etcd
  endpoints: ["${ETCD_ENDPOINT}"]
  prefix: "${ETCD_PREFIX}"

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
EOF
log_info "Config written (etcd prefix=${ETCD_PREFIX})"

log_section "Start Controller (Etcd mode)"
"$CONTROLLER_BIN" --config-file "${CONFIG_DIR}/edgion-controller.yaml" \
    > "${LOG_DIR}/controller.stdout.log" 2> "${LOG_DIR}/controller.stderr.log" &
CONTROLLER_PID=$!

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

# Auth: Etcd-mode admin secret discovery is not wired into this script
# (the secret may live as an Etcd key with backend-specific encoding).
# Inject ADMIN_TOKEN via env if your harness requires authentication.
ADMIN_TOKEN="${ADMIN_TOKEN:-}"
curl_admin() {
    if [[ -n "$ADMIN_TOKEN" ]]; then
        curl -s -H "Authorization: Bearer ${ADMIN_TOKEN}" "$@"
    else
        curl -s "$@"
    fi
}

# Probe authorization once
PROBE_STATUS=$(curl_admin -o /dev/null -w "%{http_code}" \
    "${ADMIN_URL}/api/v1/keys/namespaced/HTTPRoute")
if [[ "$PROBE_STATUS" == "401" || "$PROBE_STATUS" == "403" ]]; then
    log_warn "Admin endpoint requires auth (got $PROBE_STATUS); export ADMIN_TOKEN; SKIP"
    log_warn "Not declaring PASS — auth misconfiguration would otherwise be silenced."
    # Exit 0 so SKIP doesn't fail CI, but leave TEST_PASSED=0 so the cleanup
    # trap retains WORK_DIR for post-mortem.
    exit 0
fi

NAMESPACE="ns-list-keys-etcd"
FIXTURE_COUNT=25
log_section "Seed ${FIXTURE_COUNT} HTTPRoutes via /api/v1/namespaced/HTTPRoute/${NAMESPACE}/route-N"
for i in $(seq 1 ${FIXTURE_COUNT}); do
    BODY=$(cat <<EOF
{
  "apiVersion": "gateway.networking.k8s.io/v1",
  "kind": "HTTPRoute",
  "metadata": {"name": "route-${i}", "namespace": "${NAMESPACE}"},
  "spec": {"rules": [{"matches": [{"path": {"type": "PathPrefix", "value": "/r${i}"}}]}]}
}
EOF
)
    STATUS=$(curl_admin -X POST -H "Content-Type: application/json" \
        -d "$BODY" \
        -o /dev/null -w "%{http_code}" \
        "${ADMIN_URL}/api/v1/namespaced/HTTPRoute/${NAMESPACE}")
    [[ "$STATUS" == "200" || "$STATUS" == "201" ]] \
        || { log_error "POST route-${i} returned $STATUS"; exit 1; }
done
sleep 2

log_section "Assert /api/v1/keys/namespaced/HTTPRoute returns metadata-only list"
RESPONSE=$(curl_admin "${ADMIN_URL}/api/v1/keys/namespaced/HTTPRoute")

# Exact fixture count — proves no entry got silently dropped by projection
echo "$RESPONSE" | jq -e ".data | length == ${FIXTURE_COUNT}" >/dev/null \
    || { log_error "Expected ${FIXTURE_COUNT} items, got: $(echo "$RESPONSE" | jq -c '.data | length')"; echo "$RESPONSE" | jq .; exit 1; }

# Structural projection check: each item must have ONLY the ResourceKey-shaped
# keys (kind / apiVersion / metadata). `.spec == null` alone would be vacuous.
echo "$RESPONSE" | jq -e '.data | all((keys | sort) == ["apiVersion","kind","metadata"])' >/dev/null \
    || { log_error "Item key-set is not exactly {apiVersion, kind, metadata}; projection let extra fields through"; echo "$RESPONSE" | jq '.data[0]'; exit 1; }
echo "$RESPONSE" | jq -e '.data | all(.metadata.name != null and .metadata.name != "")' >/dev/null \
    || { log_error "Some item is missing metadata.name"; exit 1; }
echo "$RESPONSE" | jq -e '.continue_token == null or .continue_token == ""' >/dev/null \
    || { log_error "Etcd backend should not paginate — continue_token must be null"; exit 1; }
log_success "namespaced keys endpoint: ${FIXTURE_COUNT} items, ResourceKey-shaped, no pagination"

log_section "Assert /api/v1/keys/cluster/GatewayClass returns 200"
STATUS=$(curl_admin -o /tmp/list-keys-etcd-cluster.json -w "%{http_code}" \
    "${ADMIN_URL}/api/v1/keys/cluster/GatewayClass")
[[ "$STATUS" == "200" ]] \
    || { log_error "Expected 200, got $STATUS"; cat /tmp/list-keys-etcd-cluster.json; exit 1; }
jq -e '.data | type == "array"' /tmp/list-keys-etcd-cluster.json >/dev/null \
    || { log_error "cluster response data is not an array"; cat /tmp/list-keys-etcd-cluster.json; exit 1; }
log_success "cluster keys endpoint: 200 + array response"

log_section "Regression: legacy /api/v1/namespaced/HTTPRoute returns full content with spec"
LEGACY=$(curl_admin "${ADMIN_URL}/api/v1/namespaced/HTTPRoute")
echo "$LEGACY" | jq -e ".data | length == ${FIXTURE_COUNT}" >/dev/null \
    || { log_error "Legacy path lost items, got: $(echo "$LEGACY" | jq -c '.data | length')"; exit 1; }
echo "$LEGACY" | jq -e '.data | any(.spec != null)' >/dev/null \
    || { log_error "Legacy path dropped spec — regression!"; exit 1; }
log_success "Legacy namespaced path preserved"

TEST_PASSED=1
log_section "PASS: list-keys-etcd.sh"
