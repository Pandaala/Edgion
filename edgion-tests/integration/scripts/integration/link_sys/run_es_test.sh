#!/bin/bash
# =============================================================================
# Elasticsearch LinkSys Integration Test Script
#
# Standalone test for the Elasticsearch LinkSys runtime client.
# Starts a Docker Elasticsearch instance, boots Edgion (Controller + Gateway),
# loads an ES-type LinkSys CRD, and validates the full lifecycle through the
# Gateway Admin API testing endpoints.
#
# Test categories:
#   1.  Resource sync          — CRD reaches Gateway
#   2.  Client registration    — EsLinkClient created
#   3.  Health & PING          — cluster health + latency
#   4.  Index management       — create / exists / delete index
#   5.  Document CRUD          — index / get / delete documents
#   6.  Search                 — full-text search after refresh
#   7.  Bulk ingest            — send docs via bulk buffer + verify count
#   8.  Error handling         — non-existent client
#   9.  Lifecycle: delete      — delete CRD → client removed
#  10.  Lifecycle: re-create   — re-apply CRD → client restored
#
# Usage:
#   ./run_es_test.sh                  # Full run (build + test + cleanup)
#   ./run_es_test.sh --no-cleanup     # Keep services alive after test
#   ./run_es_test.sh --no-build       # Skip cargo build
#   ./run_es_test.sh --keep-alive     # Alias for --no-cleanup
# =============================================================================

set -euo pipefail

# ── Colours ─────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# ── Paths ───────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../../.." && pwd)"
CONF_SYNC_UTILS="${SCRIPT_DIR}/../../utils/conf_sync_certs.sh"
# shellcheck source=../../utils/conf_sync_certs.sh
source "$CONF_SYNC_UTILS"

ES_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/elasticsearch"
CONF_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf"
EDGION_CLI="${PROJECT_ROOT}/target/debug/edgion-cli"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
GATEWAY_BIN="${PROJECT_ROOT}/target/debug/edgion-gateway"
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.yaml"
GATEWAY_CONFIG="${PROJECT_ROOT}/config/edgion-gateway.yaml"

# ── Ports ───────────────────────────────────────────────────────────────────
CONTROLLER_ADMIN_PORT=12101
GATEWAY_ADMIN_PORT=12001
ES_PORT=19200

CONTROLLER_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
CONTROLLER_PROBE_URL="${CONTROLLER_PROBE_URL:-http://127.0.0.1:12100}"
GATEWAY_URL="http://127.0.0.1:${GATEWAY_ADMIN_PORT}"
# Gateway admin API requires a bearer token. Must be EXPORTED so the gateway
# child process (started below) inherits it and configures its admin token —
# otherwise every admin API call returns 503 "no admin token configured".
export EDGION_GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-integration-gateway-admin-token-0123456789}"
GATEWAY_PROBE_URL="${GATEWAY_PROBE_URL:-http://127.0.0.1:12000}"
ES_URL="http://127.0.0.1:${ES_PORT}"

# ── Working directory ───────────────────────────────────────────────────────
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/es_testing_${TIMESTAMP}"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"
GENERATED_SECRET_DIR="${WORK_DIR}/generated-secrets"

export EDGION_WORK_DIR="$WORK_DIR"
# Used by cert-generation helpers to write generated TLS Secrets.
export EDGION_GENERATED_SECRET_DIR="$GENERATED_SECRET_DIR"

# Token-only admin auth. The Controller reads a single pre-shared bearer token
# from EDGION_ADMIN_TOKEN. Export it (honoring an inherited value) so the Controller
# started below registers it as the allow-all admin token; edgion-cli (apply) reads
# the same value via EDGION_TOKEN.
export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
export EDGION_TOKEN="${EDGION_ADMIN_TOKEN}"

# ── Flags ───────────────────────────────────────────────────────────────────
DO_CLEANUP=true
DO_BUILD=true

# ── Counters ────────────────────────────────────────────────────────────────
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_TOTAL=0

# =============================================================================
# Argument parsing
# =============================================================================
for arg in "$@"; do
    case $arg in
        --no-cleanup|--keep-alive)
            DO_CLEANUP=false
            ;;
        --no-build)
            DO_BUILD=false
            ;;
        -h|--help)
            echo "Usage: $0 [--no-cleanup|--keep-alive] [--no-build] [-h|--help]"
            exit 0
            ;;
    esac
done

# =============================================================================
# Logging helpers
# =============================================================================
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error()   { echo -e "${RED}[✗]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[!]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}  $1${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

# =============================================================================
# Test assertion helpers
# =============================================================================
assert_eq() {
    local test_name="$1" expected="$2" actual="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if [ "$expected" = "$actual" ]; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (expected='$expected', got='$actual')"
    fi
}

assert_ne() {
    local test_name="$1" not_expected="$2" actual="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if [ "$not_expected" != "$actual" ]; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (should NOT be '$not_expected', but it is)"
    fi
}

assert_contains() {
    local test_name="$1" haystack="$2" needle="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if echo "$haystack" | grep -q "$needle"; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (missing '$needle')"
    fi
}

assert_not_contains() {
    local test_name="$1" haystack="$2" needle="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if ! echo "$haystack" | grep -q "$needle"; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (should NOT contain '$needle')"
    fi
}

assert_json_success() {
    local test_name="$1" json_resp="$2"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    local ok
    ok=$(echo "$json_resp" | jq -r '.success // false' 2>/dev/null)
    if [ "$ok" = "true" ]; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        local err
        err=$(echo "$json_resp" | jq -r '.error // "unknown"' 2>/dev/null)
        log_error "$test_name (success=false, error=$err)"
    fi
}

assert_json_failure() {
    local test_name="$1" json_resp="$2"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    local ok
    ok=$(echo "$json_resp" | jq -r 'if .success == false then "false" else "true" end' 2>/dev/null)
    if [ "$ok" = "false" ]; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (expected failure but got success)"
    fi
}

assert_ge() {
    local test_name="$1" actual="$2" threshold="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if [ "$actual" -ge "$threshold" ] 2>/dev/null; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (expected >= $threshold, got $actual)"
    fi
}

assert_gt() {
    local test_name="$1" actual="$2" threshold="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if [ "$actual" -gt "$threshold" ] 2>/dev/null; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (expected > $threshold, got $actual)"
    fi
}

# Helper: call Gateway Admin API
gw_api() {
    local method="$1" path="$2"
    shift 2
    local result
    result=$(curl -s -X "$method" "${GATEWAY_URL}${path}" \
        -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" \
        -H "Content-Type: application/json" "$@" 2>/dev/null)
    if [ -z "$result" ]; then
        echo '{"success":false,"error":"empty response"}'
    else
        echo "$result"
    fi
}

# ES client name in URL path
ES_CLIENT="default_es-test"

# =============================================================================
# Cleanup
# =============================================================================
cleanup() {
    if [ "$DO_CLEANUP" = true ]; then
        log_section "Cleanup"

        for svc in gateway controller; do
            if [ -f "${PID_DIR}/${svc}.pid" ]; then
                kill "$(cat "${PID_DIR}/${svc}.pid")" 2>/dev/null || true
            fi
        done

        pkill -f debug/edgion-gateway 2>/dev/null || true
        pkill -f debug/edgion-controller 2>/dev/null || true

        log_info "Stopping Elasticsearch container..."
        cd "$ES_DIR" && docker compose down --timeout 10 2>/dev/null || true

        log_info "Cleanup done"
    else
        log_warn "Services still running (--no-cleanup). Stop with:"
        log_warn "  kill \$(cat ${PID_DIR}/controller.pid) \$(cat ${PID_DIR}/gateway.pid)"
        log_warn "  cd $ES_DIR && docker compose down"
    fi
}
trap cleanup EXIT

# =============================================================================
# Step 0: Kill lingering processes
# =============================================================================
log_section "Cleaning up old processes"
pkill -9 -f debug/edgion-gateway 2>/dev/null || true
pkill -9 -f debug/edgion-controller 2>/dev/null || true
sleep 2
log_success "Old processes cleaned up"

# =============================================================================
# Step 1: Build (optional)
# =============================================================================
if [ "$DO_BUILD" = true ]; then
    log_section "Building Edgion binaries"
    cd "$PROJECT_ROOT"
    cargo build --bin edgion-controller --bin edgion-gateway --bin edgion-cli 2>&1 | tail -5
    log_success "Build complete"
else
    log_info "Skipping build (--no-build)"
fi

for bin in "$CONTROLLER_BIN" "$GATEWAY_BIN" "$EDGION_CLI"; do
    if [ ! -f "$bin" ]; then
        log_error "Binary not found: $bin — run without --no-build first"
        exit 1
    fi
done

# =============================================================================
# Step 2: Create work directory
# =============================================================================
log_section "Preparing work directory"
mkdir -p "$LOG_DIR" "$PID_DIR" "$CONFIG_DIR"

# conf_sync is mTLS by default (the checked-in config/edgion-*.yaml carry the cert
# paths, work-dir relative). Both binaries below run with WORK_DIR as their work
# dir, so the cert set has to land there.
conf_sync_distribute_certs "$WORK_DIR"

if [ -d "${PROJECT_ROOT}/config/crd" ]; then
    cp -r "${PROJECT_ROOT}/config/crd" "$CONFIG_DIR/"
    log_success "CRD schemas copied"
fi

# =============================================================================
# Step 3: Start Elasticsearch (Docker)
# =============================================================================
log_section "Starting Elasticsearch"

# Generate the backend CA + server cert that the nginx TLS terminator
# (es-tls-proxy) mounts. Required before `docker compose up` so the volume mount
# at edgion-tests/certs/backend is populated. Drives the P0#3 outbound-TLS case.
BACKEND_CERT_SCRIPT="${SCRIPT_DIR}/../../gen_certs/generate_backend_certs.sh"
MTLS_CERT_SCRIPT="${SCRIPT_DIR}/../../gen_certs/generate_mtls_certs.sh"
BACKEND_CA_CRT="${PROJECT_ROOT}/edgion-tests/certs/backend/ca.crt"
MTLS_CLIENT_CRT="${PROJECT_ROOT}/edgion-tests/certs/mtls/valid-client.crt"
MTLS_CLIENT_KEY="${PROJECT_ROOT}/edgion-tests/certs/mtls/valid-client.key"
log_info "Generating backend + mTLS certs for the ES TLS terminators..."
bash "$BACKEND_CERT_SCRIPT" > /dev/null 2>&1 || true
bash "$MTLS_CERT_SCRIPT" > /dev/null 2>&1 || true
if [ ! -f "$BACKEND_CA_CRT" ] || [ ! -f "$MTLS_CLIENT_CRT" ] || [ ! -f "$MTLS_CLIENT_KEY" ]; then
    log_error "TLS certs not found after generation (backend CA / mTLS client cert)"
    exit 1
fi
log_success "Backend + mTLS certs ready"

cd "$ES_DIR"
docker compose pull --quiet 2>/dev/null || true
docker compose up -d 2>&1 | grep -v "^$" || true

log_info "Waiting for Elasticsearch to be healthy (this may take 30-60s)..."
for i in $(seq 1 90); do
    if curl -sf "${ES_URL}/_cluster/health" > /dev/null 2>&1; then
        CLUSTER_STATUS=$(curl -s "${ES_URL}/_cluster/health" | jq -r '.status // "unknown"')
        log_success "Elasticsearch is ready (status=${CLUSTER_STATUS}, waited ${i}s)"
        break
    fi
    if [ "$i" -eq 90 ]; then
        log_error "Elasticsearch failed to start within 90s"
        docker compose logs
        exit 1
    fi
    sleep 1
done

# =============================================================================
# Step 4: Start Controller
# =============================================================================
log_section "Starting Controller"
cd "$PROJECT_ROOT"

"$CONTROLLER_BIN" \
    -c "$CONTROLLER_CONFIG" \
    --work-dir "$WORK_DIR" \
    --conf-dir "$CONFIG_DIR" \
    --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
    --test-mode \
    > "${LOG_DIR}/controller.log" 2>&1 &
echo $! > "${PID_DIR}/controller.pid"
log_info "Controller PID: $(cat "${PID_DIR}/controller.pid")"

for i in $(seq 1 30); do
    if curl -sf "${CONTROLLER_PROBE_URL}/ready" > /dev/null 2>&1; then
        log_success "Controller ready (waited ${i}s)"
        break
    fi
    if [ "$i" -eq 30 ]; then
        log_error "Controller failed to start"
        tail -30 "${LOG_DIR}/controller.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
done

# =============================================================================
# Step 5: Load configuration
# =============================================================================
log_section "Loading configuration"

for f in "${CONF_DIR}/base"/*.yaml; do
    "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$f" > /dev/null 2>&1 || true
done
log_success "Base config loaded"

# CA Secret for the outbound-TLS LinkSys (es-tls-test, caSource: SecretRef).
# Resolved by the Controller in the LinkSys namespace (default) — same-namespace,
# so no ReferenceGrant is needed. Applied BEFORE the LinkSys CRDs so the caSecretRef
# resolves on first reconcile (avoids a Pending->ready re-reconcile race).
ES_CA_B64=$(base64 < "$BACKEND_CA_CRT" | tr -d '\n')
ES_CA_SECRET_YAML="${CONFIG_DIR}/Secret_default_es-backend-ca.yaml"
cat > "$ES_CA_SECRET_YAML" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: es-backend-ca
  namespace: default
type: Opaque
data:
  ca.crt: ${ES_CA_B64}
EOF
"$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$ES_CA_SECRET_YAML" 2>&1
log_success "ES backend CA Secret (default/es-backend-ca) loaded"

# Client-identity Secret for the mutual-TLS LinkSys (es-mtls-test, clientSecretRef).
# kubernetes.io/tls Secret carrying the valid client cert/key signed by the mTLS
# client CA that the nginx mutual-TLS terminator trusts.
ES_CLIENT_CRT_B64=$(base64 < "$MTLS_CLIENT_CRT" | tr -d '\n')
ES_CLIENT_KEY_B64=$(base64 < "$MTLS_CLIENT_KEY" | tr -d '\n')
ES_CLIENT_SECRET_YAML="${CONFIG_DIR}/Secret_default_es-client-cert.yaml"
cat > "$ES_CLIENT_SECRET_YAML" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: es-client-cert
  namespace: default
type: kubernetes.io/tls
data:
  tls.crt: ${ES_CLIENT_CRT_B64}
  tls.key: ${ES_CLIENT_KEY_B64}
EOF
"$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$ES_CLIENT_SECRET_YAML" 2>&1
log_success "ES mTLS client cert Secret (default/es-client-cert) loaded"

# Global outboundTls default for the inheritance scenario (es-tls-inherit, P1#5).
# Override the base EdgionGatewayConfig (same name -> upsert) by appending an
# outboundTls block while keeping every original field. Cluster-scoped CA
# references require an explicit namespace, so this points to the generated
# Secret in `default`. This affects only this standalone run; the shared base
# file is left untouched. Other ES scenarios set local overrides, so the global
# default only changes the behavior of the no-local-CA es-tls-inherit client.
# The filesystem config center derives identity from the filename, so this must
# follow Kind_name.yaml instead of using a descriptive suffix.
EGC_OVERRIDE_YAML="${CONFIG_DIR}/EdgionGatewayConfig_example-gateway.yaml"
cp "${CONF_DIR}/base/EdgionGatewayConfig.yaml" "$EGC_OVERRIDE_YAML"
cat >> "$EGC_OVERRIDE_YAML" <<'EOF'

  # Appended by run_es_test.sh for the global outboundTls inheritance scenario.
  outboundTls:
    validation:
      caCertificateRefs:
        - kind: Secret
          name: es-backend-ca
          namespace: default
EOF
"$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$EGC_OVERRIDE_YAML" 2>&1
log_success "EdgionGatewayConfig outboundTls global CA reference loaded"

for f in $(ls "${CONF_DIR}/LinkSys/Elasticsearch"/*.yaml | sort); do
    "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$f" 2>&1
done
log_success "LinkSys/Elasticsearch config loaded"

sleep 1

# =============================================================================
# Step 6: Start Gateway
# =============================================================================
log_section "Starting Gateway"

EDGION_ACCESS_LOG="${LOG_DIR}/edgion_access.log" \
EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/edgion_access.log" \
EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
"$GATEWAY_BIN" \
    -c "$GATEWAY_CONFIG" \
    --work-dir "$WORK_DIR" \
    --integration-testing-mode \
    > "${LOG_DIR}/gateway.log" 2>&1 &
echo $! > "${PID_DIR}/gateway.pid"
log_info "Gateway PID: $(cat "${PID_DIR}/gateway.pid")"

for i in $(seq 1 60); do
    if curl -sf "${GATEWAY_PROBE_URL}/ready" > /dev/null 2>&1; then
        log_success "Gateway ready (waited ${i}s)"
        break
    fi
    if [ "$i" -eq 60 ]; then
        log_error "Gateway failed to start"
        tail -30 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
done

for i in $(seq 1 15); do
    if grep -q "LB preload completed" "${LOG_DIR}/gateway.log" 2>/dev/null; then
        log_info "LB preload verified (waited ${i}s)"
        break
    fi
    sleep 1
done

# Give LinkSys ConfHandler time to initialise the ES client
sleep 5

# =============================================================================
# ──────────────────────────── TEST SUITE ────────────────────────────────────
# =============================================================================

log_section "Test Suite: Elasticsearch LinkSys Integration"

# ── 1. Resource Sync ────────────────────────────────────────────────────────
log_info "─── 1. Resource Sync ───"

RESP=$(gw_api GET "/configclient/LinkSys?namespace=default&name=es-test")
assert_json_success "LinkSys CRD (es-test) synced to Gateway" "$RESP"

# ── 2. Client Registration ─────────────────────────────────────────────────
log_info "─── 2. Client Registration ───"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/clients")
assert_json_success "ES clients endpoint reachable" "$RESP"
assert_contains "es-test client registered" "$RESP" "default/es-test"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/info")
assert_json_success "es-test info reachable" "$RESP"
assert_contains "Client has endpoint" "$RESP" "19200"

# ── 3. Health & PING ───────────────────────────────────────────────────────
log_info "─── 3. Health & PING ───"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/ping")
assert_json_success "PING (cluster health) succeeds" "$RESP"
LATENCY=$(echo "$RESP" | jq -r '.data.latency_ms // -1')
assert_ge "PING latency valid (${LATENCY}ms)" "$LATENCY" 0

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/health")
assert_json_success "Single client health check" "$RESP"
CONNECTED=$(echo "$RESP" | jq -r '.data.connected // false')
assert_eq "Client connected=true" "true" "$CONNECTED"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/health")
assert_json_success "All-clients health check" "$RESP"

# ── 3.5 Outbound TLS: caSource SecretRef (private-CA REPLACE) ───────────────
# es-tls-test connects to the nginx TLS terminator (https://127.0.0.1:19201)
# whose server cert is signed by the backend test CA. caSource: SecretRef
# REPLACES the public roots with that private CA, so a successful PING proves the
# connector actually completed a TLS handshake trusting only the private CA.
# es-tls-systemca is the negative control: same endpoint, caSource: System →
# the private CA is not in the public roots → handshake fails → never connects.
log_info "─── 3.5 Outbound TLS (caSource: SecretRef, CA REPLACE) ───"
ES_TLS_CLIENT="default_es-tls-test"
ES_TLS_SYSTEMCA_CLIENT="default_es-tls-systemca"

RESP=$(gw_api GET "/configclient/LinkSys?namespace=default&name=es-tls-test")
assert_json_success "es-tls-test CRD synced to Gateway" "$RESP"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/clients")
assert_contains "es-tls-test client registered (CA Secret resolved)" "$RESP" "default/es-tls-test"

# Positive: private CA trusted over TLS → PING succeeds.
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_TLS_CLIENT}/ping")
assert_json_success "es-tls-test PING over TLS succeeds (private CA REPLACE trusted)" "$RESP"
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_TLS_CLIENT}/health")
assert_eq "es-tls-test connected=true over TLS" "true" "$(echo "$RESP" | jq -r '.data.connected // false')"

# Negative control: System roots only → the private CA is untrusted → PING fails.
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_TLS_SYSTEMCA_CLIENT}/ping")
assert_json_failure "es-tls-systemca PING fails (System roots do not trust the private CA)" "$RESP"
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_TLS_SYSTEMCA_CLIENT}/health")
assert_ne "es-tls-systemca never connects (proves SecretRef REPLACES public roots)" "true" "$(echo "$RESP" | jq -r '.data.connected // false')"

# ── 3.6 Outbound TLS: verify=false honored (M-A1, no env gate) ──────────────
# es-tls-noverify hits the SAME private-cert endpoint with NO CA and verify=false.
# Elasticsearch is a reqwest provider (provider_can_disable=true), so the resolver
# honors verify=false instead of rejecting it — and after M-A1 there is no
# production/EDGION_ENV gate. The handshake to the untrusted private cert must
# therefore succeed, in direct contrast to es-tls-systemca (verify on → fails).
log_info "─── 3.6 Outbound TLS (verify=false honored, M-A1) ───"
ES_TLS_NOVERIFY_CLIENT="default_es-tls-noverify"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/clients")
assert_contains "es-tls-noverify client registered" "$RESP" "default/es-tls-noverify"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_TLS_NOVERIFY_CLIENT}/ping")
assert_json_success "es-tls-noverify PING succeeds (verify=false honored, no CA)" "$RESP"
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_TLS_NOVERIFY_CLIENT}/health")
assert_eq "es-tls-noverify connected=true (verify=false skips cert validation)" "true" "$(echo "$RESP" | jq -r '.data.connected // false')"

# ── 3.7 Outbound mTLS: clientSecretRef (client identity presented) ──────────
# es-mtls-test connects to the mutual-TLS terminator (https://127.0.0.1:19202,
# ssl_verify_client on). It trusts the server cert via caSecretRef AND presents a
# client cert via clientSecretRef. A successful PING proves the connector actually
# completed a mutual-TLS handshake. es-mtls-noclient is the negative control: same
# endpoint, no client identity → nginx rejects the handshake → never connects.
log_info "─── 3.7 Outbound mTLS (clientSecretRef) ───"
ES_MTLS_CLIENT="default_es-mtls-test"
ES_MTLS_NOCLIENT="default_es-mtls-noclient"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/clients")
assert_contains "es-mtls-test client registered (CA + client cert resolved)" "$RESP" "default/es-mtls-test"

# Positive: valid client cert presented → mutual-TLS handshake succeeds.
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_MTLS_CLIENT}/ping")
assert_json_success "es-mtls-test PING over mTLS succeeds (valid client cert)" "$RESP"
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_MTLS_CLIENT}/health")
assert_eq "es-mtls-test connected=true over mTLS" "true" "$(echo "$RESP" | jq -r '.data.connected // false')"

# Negative control: no client identity → the mutual-TLS server rejects it.
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_MTLS_NOCLIENT}/ping")
assert_json_failure "es-mtls-noclient PING fails (no client cert presented)" "$RESP"
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_MTLS_NOCLIENT}/health")
assert_ne "es-mtls-noclient never connects (mutual TLS requires a client cert)" "true" "$(echo "$RESP" | jq -r '.data.connected // false')"

# ── 3.8 Outbound TLS: global outboundTls inheritance ────────────────────────
# es-tls-inherit has tls.enabled but NO local CA. It inherits the cluster-wide
# EdgionGatewayConfig.outboundTls validation CA reference and connects
# to the private-cert endpoint. A successful PING proves the global CA default was
# inherited; es-tls-systemca (local caSource: System) still fails, proving a local
# override beats the global default.
log_info "─── 3.8 Outbound TLS (global outboundTls inheritance) ───"
ES_TLS_INHERIT_CLIENT="default_es-tls-inherit"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/clients")
assert_contains "es-tls-inherit client registered" "$RESP" "default/es-tls-inherit"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_TLS_INHERIT_CLIENT}/ping")
assert_json_success "es-tls-inherit PING succeeds (inherited global CA default)" "$RESP"
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_TLS_INHERIT_CLIENT}/health")
assert_eq "es-tls-inherit connected=true (global outboundTls CA inherited)" "true" "$(echo "$RESP" | jq -r '.data.connected // false')"

# ── 4. Index Management ───────────────────────────────────────────────────
log_info "─── 4. Index Management ───"

TEST_INDEX="edgion-integration-test"

# Create index
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/create-index/${TEST_INDEX}")
assert_json_success "Create index" "$RESP"

# Check index exists
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/index-exists/${TEST_INDEX}")
assert_json_success "Index exists check" "$RESP"
assert_eq "Index exists" "true" "$(echo "$RESP" | jq -r '.data.exists // false')"

# Create index again (should be idempotent)
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/create-index/${TEST_INDEX}")
assert_json_success "Create index (idempotent)" "$RESP"

# Delete index
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/delete-index/${TEST_INDEX}")
assert_json_success "Delete index" "$RESP"

# Check index no longer exists
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/index-exists/${TEST_INDEX}")
assert_json_success "Index exists after delete" "$RESP"
assert_eq "Index no longer exists" "false" "$(echo "$RESP" | jq -r '.data.exists | tostring')"

# Delete non-existent index (should be idempotent)
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/delete-index/${TEST_INDEX}")
assert_json_success "Delete index (idempotent)" "$RESP"

# ── 5. Document CRUD ─────────────────────────────────────────────────────
log_info "─── 5. Document CRUD ───"

DOC_INDEX="edgion-doc-test"

# Create index for doc tests
gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/create-index/${DOC_INDEX}" > /dev/null

# Index a document
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/index-doc/${DOC_INDEX}" \
    -d '{"message":"hello edgion","level":"info","timestamp":"2026-02-13T12:00:00Z"}')
assert_json_success "Index document" "$RESP"
DOC_ID=$(echo "$RESP" | jq -r '.data.doc_id // ""')
assert_ne "Document ID is non-empty" "" "$DOC_ID"

# Get document by ID
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/get-doc/${DOC_INDEX}/${DOC_ID}")
assert_json_success "Get document by ID" "$RESP"
assert_eq "Document message matches" "hello edgion" "$(echo "$RESP" | jq -r '.data.source.message // ""')"
assert_eq "Document level matches" "info" "$(echo "$RESP" | jq -r '.data.source.level // ""')"

# Index second document
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/index-doc/${DOC_INDEX}" \
    -d '{"message":"second doc","level":"warn","timestamp":"2026-02-13T12:01:00Z"}')
assert_json_success "Index second document" "$RESP"
DOC_ID_2=$(echo "$RESP" | jq -r '.data.doc_id // ""')

# Delete first document
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/delete-doc/${DOC_INDEX}/${DOC_ID}")
assert_json_success "Delete document" "$RESP"
assert_eq "Document deleted" "true" "$(echo "$RESP" | jq -r '.data.deleted // false')"

# Get deleted document → null
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/get-doc/${DOC_INDEX}/${DOC_ID}")
assert_json_success "Get deleted document" "$RESP"
assert_eq "Deleted doc source is null" "null" "$(echo "$RESP" | jq -r '.data.source')"

# Delete non-existent document
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/delete-doc/${DOC_INDEX}/nonexistent-id-12345")
assert_json_success "Delete non-existent document" "$RESP"
assert_eq "Non-existent doc not deleted" "false" "$(echo "$RESP" | jq -r '.data.deleted | tostring')"

# ── 6. Search ────────────────────────────────────────────────────────────
log_info "─── 6. Search ───"

# Index a few more docs for search
gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/index-doc/${DOC_INDEX}" \
    -d '{"message":"search test alpha","level":"info"}' > /dev/null
gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/index-doc/${DOC_INDEX}" \
    -d '{"message":"search test beta","level":"error"}' > /dev/null
gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/index-doc/${DOC_INDEX}" \
    -d '{"message":"search test gamma","level":"info"}' > /dev/null

# Refresh index to make docs searchable
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/refresh/${DOC_INDEX}")
assert_json_success "Refresh index" "$RESP"

# Search all
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/search/${DOC_INDEX}" \
    -d '{"query":{"match_all":{}}}')
assert_json_success "Search all documents" "$RESP"
TOTAL=$(echo "$RESP" | jq -r '.data.total // 0')
assert_ge "Search found at least 4 docs" "$TOTAL" 4

# Search by term
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/search/${DOC_INDEX}" \
    -d '{"query":{"match":{"level":"error"}}}')
assert_json_success "Search by level=error" "$RESP"
ERROR_TOTAL=$(echo "$RESP" | jq -r '.data.total // 0')
assert_ge "Found at least 1 error doc" "$ERROR_TOTAL" 1

# Search by message
RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/search/${DOC_INDEX}" \
    -d '{"query":{"match":{"message":"alpha"}}}')
assert_json_success "Search by message=alpha" "$RESP"
assert_ge "Found alpha doc" "$(echo "$RESP" | jq -r '.data.total // 0')" 1

# Count
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/count/${DOC_INDEX}")
assert_json_success "Count documents" "$RESP"
COUNT=$(echo "$RESP" | jq -r '.data.count // 0')
assert_ge "Count >= 4" "$COUNT" 4

# Cleanup doc index
gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/delete-index/${DOC_INDEX}" > /dev/null

# ── 7. Bulk Ingest ──────────────────────────────────────────────────────
log_info "─── 7. Bulk Ingest ───"

# Bulk ingest writes to the date-based index configured in the CRD:
# prefix = "edgion-test", datePattern = "%Y.%m.%d" → "edgion-test-YYYY.MM.DD"
# The CRD sets a tiny maxBodyBytes (256), so these 10 docs are flushed as
# several sub-batches (F-002 partitioning); the count check below would catch
# any sub-batch that the split path lost.
BULK_INDEX="edgion-test-$(date +%Y.%m.%d)"
log_info "Bulk target index: ${BULK_INDEX}"

# Send 10 docs via bulk
BULK_DOCS='{"docs":['
for i in $(seq 1 10); do
    if [ "$i" -gt 1 ]; then BULK_DOCS="${BULK_DOCS},"; fi
    BULK_DOCS="${BULK_DOCS}{\"message\":\"bulk-doc-${i}\",\"seq\":${i}}"
done
BULK_DOCS="${BULK_DOCS}]}"

RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/bulk" -d "$BULK_DOCS")
assert_json_success "Bulk send 10 docs" "$RESP"
SENT=$(echo "$RESP" | jq -r '.data.sent // 0')
assert_eq "All 10 docs sent to buffer" "10" "$SENT"

# Wait for bulk flush (flush interval is 2s in test config)
log_info "Waiting 4s for bulk flush..."
sleep 4

# Refresh and count (bulk writes to the date-based index)
gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/refresh/${BULK_INDEX}" > /dev/null
RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/count/${BULK_INDEX}")
assert_json_success "Count after bulk" "$RESP"
BULK_COUNT=$(echo "$RESP" | jq -r '.data.count // 0')
assert_ge "Bulk: at least 10 docs indexed" "$BULK_COUNT" 10

# Cleanup
gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/delete-index/${BULK_INDEX}" > /dev/null

# ── 8. Error Handling ──────────────────────────────────────────────────
log_info "─── 8. Error Handling ───"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/nons_noname/ping")
assert_json_failure "PING non-existent client returns error" "$RESP"
assert_contains "Error mentions 'not found'" "$RESP" "not found"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/nons_noname/health")
assert_json_failure "Health non-existent client returns error" "$RESP"

RESP=$(gw_api POST "/api/v1/testing/link-sys/es/nons_noname/create-index/test")
assert_json_failure "Create index on non-existent client returns error" "$RESP"

# ── 9. Lifecycle: Delete CRD ──────────────────────────────────────────
log_info "─── 9. Lifecycle: Delete LinkSys ───"

"$EDGION_CLI" --server "$CONTROLLER_URL" delete LinkSys es-test -n default 2>&1 || true
sleep 5

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/clients")
CLIENTS_JSON=$(echo "$RESP" | jq -r '.data // []')
assert_not_contains "es-test removed after delete" "$CLIENTS_JSON" "default/es-test"

# ── 10. Lifecycle: Re-create ──────────────────────────────────────────
log_info "─── 10. Lifecycle: Re-create LinkSys ───"

"$EDGION_CLI" --server "$CONTROLLER_URL" apply \
    -f "${CONF_DIR}/LinkSys/Elasticsearch/01_LinkSys_default_es-test.yaml" 2>&1 || true
sleep 5

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/clients")
assert_contains "es-test restored after re-create" "$RESP" "default/es-test"

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/ping")
assert_json_success "Re-created client PING succeeds" "$RESP"

# Verify re-created client works (index + get + delete)
RE_INDEX="edgion-recreate-test"
gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/create-index/${RE_INDEX}" > /dev/null

RESP=$(gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/index-doc/${RE_INDEX}" \
    -d '{"message":"alive after recreate"}')
assert_json_success "Re-created client can index docs" "$RESP"

gw_api POST "/api/v1/testing/link-sys/es/${ES_CLIENT}/delete-index/${RE_INDEX}" > /dev/null

# =============================================================================
# ──────────────────────────── REPORT ────────────────────────────────────────
# =============================================================================
log_section "Test Report"

echo -e "  Total:   ${TESTS_TOTAL}"
echo -e "  Passed:  ${GREEN}${TESTS_PASSED}${NC}"
echo -e "  Failed:  ${RED}${TESTS_FAILED}${NC}"
echo ""

cat > "${WORK_DIR}/report.log" <<EOF
Elasticsearch LinkSys Integration Test Report
$(date)
Total:  ${TESTS_TOTAL}
Passed: ${TESTS_PASSED}
Failed: ${TESTS_FAILED}
EOF

if [ "$TESTS_FAILED" -gt 0 ]; then
    log_error "Some tests FAILED!"
    echo ""
    log_info "Debug tips:"
    log_info "  Gateway log:    ${LOG_DIR}/gateway.log"
    log_info "  Controller log: ${LOG_DIR}/controller.log"
    exit 1
else
    log_success "All tests PASSED!"
    exit 0
fi
