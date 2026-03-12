#!/bin/bash
# =============================================================================
# Etcd LinkSys Integration Test Script
#
# Standalone test for the Etcd LinkSys runtime client.
# Starts a Docker Etcd instance, boots Edgion (Controller + Gateway), loads
# Etcd-type LinkSys CRDs (one plain, one with namespace), and validates the
# full lifecycle through the Gateway Admin API testing endpoints.
#
# Test categories:
#   1.  Resource sync          — CRD reaches Gateway
#   2.  Client registration    — EtcdLinkClient created (plain + namespaced)
#   3.  Health & PING          — connectivity + latency
#   4.  KV operations          — PUT / GET / DELETE
#   5.  PUT overwrite          — PUT same key twice, verify new value
#   6.  Prefix operations      — GET prefix / DELETE prefix
#   7.  Lease operations       — GRANT / TTL / REVOKE / key auto-delete
#   8.  Lease auto-expire      — short lease expires → key removed
#   9.  Distributed lock       — acquire + release (etcd native Lock API)
#  10.  Namespace isolation    — namespaced client auto-adds prefix
#  11.  Error handling         — non-existent client
#  12.  Lifecycle: delete      — delete CRD → client removed
#  13.  Lifecycle: re-create   — re-apply CRD → client restored
#
# Usage:
#   ./run_etcd_test.sh                  # Full run (build + test + cleanup)
#   ./run_etcd_test.sh --no-cleanup     # Keep services alive after test
#   ./run_etcd_test.sh --no-build       # Skip cargo build
#   ./run_etcd_test.sh --keep-alive     # Alias for --no-cleanup
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
UTILS_DIR="${SCRIPT_DIR}/../utils"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

ETCD_DIR="${PROJECT_ROOT}/examples/test/conf/Services/etcd"
CONF_DIR="${PROJECT_ROOT}/examples/test/conf"
EDGION_CTL="${PROJECT_ROOT}/target/debug/edgion-ctl"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
GATEWAY_BIN="${PROJECT_ROOT}/target/debug/edgion-gateway"
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.toml"
GATEWAY_CONFIG="${PROJECT_ROOT}/config/edgion-gateway.toml"

# ── Ports ───────────────────────────────────────────────────────────────────
CONTROLLER_ADMIN_PORT=5800
GATEWAY_ADMIN_PORT=5900
ETCD_PORT=12379

CONTROLLER_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
GATEWAY_URL="http://127.0.0.1:${GATEWAY_ADMIN_PORT}"

# ── Working directory (timestamped, same pattern as start_all_with_conf.sh) ─
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/etcd_testing_${TIMESTAMP}"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"

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
# Logging helpers (same style as start_all_with_conf.sh)
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
        log_error "$test_name (should NOT contain '$needle', but it does)"
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
    # NOTE: cannot use jq's `// true` (alternative operator) here because
    # it treats `false` the same as `null` and would return `true`.
    ok=$(echo "$json_resp" | jq -r 'if .success == false then "false" else "true" end' 2>/dev/null)
    if [ "$ok" = "false" ]; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (expected failure but got success)"
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

# Helper: call Gateway Admin API (returns JSON or error stub)
gw_api() {
    local method="$1" path="$2"
    shift 2
    local result
    result=$(curl -s -X "$method" "${GATEWAY_URL}${path}" \
        -H "Content-Type: application/json" "$@" 2>/dev/null)
    if [ -z "$result" ]; then
        echo '{"success":false,"error":"empty response"}'
    else
        echo "$result"
    fi
}

# Etcd client names in URL path: "namespace_name" (underscore-separated)
ETCD_CLIENT="default_etcd-test"
ETCD_NS_CLIENT="default_etcd-ns-test"

# =============================================================================
# Cleanup (trap on EXIT, same pattern as run_redis_test.sh)
# =============================================================================
cleanup() {
    if [ "$DO_CLEANUP" = true ]; then
        log_section "Cleanup"

        # Kill Edgion processes by PID file
        for svc in gateway controller; do
            if [ -f "${PID_DIR}/${svc}.pid" ]; then
                kill "$(cat "${PID_DIR}/${svc}.pid")" 2>/dev/null || true
            fi
        done

        # Fallback: kill by pattern (same as kill_all.sh)
        pkill -f edgion-gateway 2>/dev/null || true
        pkill -f edgion-controller 2>/dev/null || true

        # Stop Etcd
        log_info "Stopping Etcd container..."
        cd "$ETCD_DIR" && docker compose down --timeout 5 2>/dev/null || true

        log_info "Cleanup done"
    else
        log_warn "Services still running (--no-cleanup). Stop with:"
        log_warn "  kill \$(cat ${PID_DIR}/controller.pid) \$(cat ${PID_DIR}/gateway.pid)"
        log_warn "  cd $ETCD_DIR && docker compose down"
    fi
}
trap cleanup EXIT

# =============================================================================
# Step 0: Kill lingering processes
# =============================================================================
log_section "Cleaning up old processes"
pkill -9 -f edgion-gateway 2>/dev/null || true
pkill -9 -f edgion-controller 2>/dev/null || true
sleep 2
log_success "Old processes cleaned up"

# =============================================================================
# Step 1: Build (optional)
# =============================================================================
if [ "$DO_BUILD" = true ]; then
    log_section "Building Edgion binaries"
    cd "$PROJECT_ROOT"
    cargo build --bin edgion-controller --bin edgion-gateway --bin edgion-ctl 2>&1 | tail -5
    log_success "Build complete"
else
    log_info "Skipping build (--no-build)"
fi

for bin in "$CONTROLLER_BIN" "$GATEWAY_BIN" "$EDGION_CTL"; do
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

# Copy CRD schemas
if [ -d "${PROJECT_ROOT}/config/crd" ]; then
    cp -r "${PROJECT_ROOT}/config/crd" "$CONFIG_DIR/"
    log_success "CRD schemas copied"
fi

# =============================================================================
# Step 3: Start Etcd (Docker)
# =============================================================================
log_section "Starting Etcd"
cd "$ETCD_DIR"
docker compose pull --quiet 2>/dev/null || true
docker compose up -d 2>&1 | grep -v "^$" || true

log_info "Waiting for Etcd to be healthy..."
for i in $(seq 1 30); do
    if docker exec edgion-test-etcd etcdctl endpoint health 2>/dev/null | grep -q "is healthy"; then
        log_success "Etcd is ready (waited ${i}s)"
        break
    fi
    if [ "$i" -eq 30 ]; then
        log_error "Etcd failed to start within 30s"
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

# Wait for controller ready
for i in $(seq 1 30); do
    if curl -sf "${CONTROLLER_URL}/ready" > /dev/null 2>&1; then
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
# Step 5: Load base config + LinkSys/Etcd config
# =============================================================================
log_section "Loading configuration"

# Base config (GatewayClass, EdgionGatewayConfig, etc.)
for f in "${CONF_DIR}/base"/*.yaml; do
    "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$f" > /dev/null 2>&1 || true
done
log_success "Base config loaded"

# LinkSys/Etcd config (Gateway resource + LinkSys CRDs — sorted, so 00_Gateway loads first)
for f in $(ls "${CONF_DIR}/LinkSys/Etcd"/*.yaml | sort); do
    "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$f" 2>&1
done
log_success "LinkSys/Etcd config loaded (plain + namespaced)"

sleep 1

# =============================================================================
# Step 6: Start Gateway
# =============================================================================
log_section "Starting Gateway"

EDGION_ACCESS_LOG="${LOG_DIR}/access.log" \
EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log" \
"$GATEWAY_BIN" \
    -c "$GATEWAY_CONFIG" \
    --work-dir "$WORK_DIR" \
    --integration-testing-mode \
    > "${LOG_DIR}/gateway.log" 2>&1 &
echo $! > "${PID_DIR}/gateway.pid"
log_info "Gateway PID: $(cat "${PID_DIR}/gateway.pid")"

# Wait for ready
for i in $(seq 1 60); do
    if curl -sf "${GATEWAY_URL}/ready" > /dev/null 2>&1; then
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

# Wait for LB preload
for i in $(seq 1 15); do
    if grep -q "LB preload completed" "${LOG_DIR}/gateway.log" 2>/dev/null; then
        log_info "LB preload verified (waited ${i}s)"
        break
    fi
    sleep 1
done

# Give LinkSys ConfHandler time to initialise the Etcd clients
sleep 3

# =============================================================================
# ──────────────────────────── TEST SUITE ────────────────────────────────────
# =============================================================================

log_section "Test Suite: Etcd LinkSys Integration"

# ── 1. Resource sync ────────────────────────────────────────────────────────
log_info "─── 1. Resource Sync ───"

RESP=$(gw_api GET "/configclient/LinkSys?namespace=default&name=etcd-test")
assert_json_success "LinkSys CRD (etcd-test) synced to Gateway" "$RESP"

RESP=$(gw_api GET "/configclient/LinkSys?namespace=default&name=etcd-ns-test")
assert_json_success "LinkSys CRD (etcd-ns-test) synced to Gateway" "$RESP"

# ── 2. Client registration ─────────────────────────────────────────────────
log_info "─── 2. Client Registration ───"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/clients")
assert_json_success "Etcd clients endpoint reachable" "$RESP"
assert_contains "etcd-test client registered" "$RESP" "default/etcd-test"
assert_contains "etcd-ns-test client registered" "$RESP" "default/etcd-ns-test"

# Check client info (plain — no namespace)
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/info")
assert_json_success "etcd-test info reachable" "$RESP"
NAMESPACE_VAL=$(echo "$RESP" | jq -r '.data.namespace // ""')
# plain client has empty or null namespace
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if [ -z "$NAMESPACE_VAL" ] || [ "$NAMESPACE_VAL" = "null" ] || [ "$NAMESPACE_VAL" = "" ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_success "etcd-test has empty namespace (plain)"
else
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_error "etcd-test namespace should be empty, got '$NAMESPACE_VAL'"
fi

# Check client info (namespaced)
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/info")
assert_json_success "etcd-ns-test info reachable" "$RESP"
assert_contains "etcd-ns-test has namespace" "$(echo "$RESP" | jq -r '.data.namespace // ""')" "edgion-test"

# ── 3. Health & PING ───────────────────────────────────────────────────────
log_info "─── 3. Health & PING ───"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/ping")
assert_json_success "PING (status) succeeds" "$RESP"
LATENCY=$(echo "$RESP" | jq -r '.data.latency_ms // -1')
assert_ge "PING latency valid (${LATENCY}ms)" "$LATENCY" 0

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/health")
assert_json_success "Single client health check" "$RESP"
CONNECTED=$(echo "$RESP" | jq -r '.data.connected // false')
assert_eq "Client connected=true" "true" "$CONNECTED"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/health")
assert_json_success "All-clients health check" "$RESP"

# Also check namespaced client health
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/ping")
assert_json_success "PING namespaced client" "$RESP"

# ── 4. KV Operations ──────────────────────────────────────────────────────
log_info "─── 4. KV Operations ───"

# PUT
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d '{"key":"edgion/test/kv","value":"hello-edgion"}')
assert_json_success "PUT key" "$RESP"

# GET
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Ftest%2Fkv")
assert_json_success "GET key" "$RESP"
assert_eq "GET value matches" "hello-edgion" "$(echo "$RESP" | jq -r '.data.value // ""')"

# PUT another key
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d '{"key":"edgion/test/kv2","value":"second-value"}')
assert_json_success "PUT second key" "$RESP"

# GET second key
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Ftest%2Fkv2")
assert_eq "GET second value" "second-value" "$(echo "$RESP" | jq -r '.data.value // ""')"

# DELETE
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/delete/edgion%2Ftest%2Fkv")
assert_json_success "DELETE key" "$RESP"
assert_eq "DELETE count is 1" "1" "$(echo "$RESP" | jq -r '.data.deleted // 0')"

# GET after DELETE → null
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Ftest%2Fkv")
assert_json_success "GET after DELETE succeeds" "$RESP"
assert_eq "GET after DELETE returns null" "null" "$(echo "$RESP" | jq -r '.data.value')"

# Cleanup kv2
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/delete/edgion%2Ftest%2Fkv2" > /dev/null

# ── 5. PUT Overwrite ─────────────────────────────────────────────────────
log_info "─── 5. PUT Overwrite ───"

RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d '{"key":"edgion/test/overwrite","value":"original"}')
assert_json_success "PUT original value" "$RESP"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Ftest%2Foverwrite")
assert_eq "GET original value" "original" "$(echo "$RESP" | jq -r '.data.value // ""')"

# Overwrite same key
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d '{"key":"edgion/test/overwrite","value":"updated"}')
assert_json_success "PUT updated value (overwrite)" "$RESP"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Ftest%2Foverwrite")
assert_eq "GET after overwrite returns new value" "updated" "$(echo "$RESP" | jq -r '.data.value // ""')"

# Cleanup
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/delete/edgion%2Ftest%2Foverwrite" > /dev/null

# ── 6. Prefix Operations ────────────────────────────────────────────────────
log_info "─── 6. Prefix Operations ───"

# Setup: put multiple keys under prefix
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d '{"key":"edgion/prefix/a","value":"val-a"}' > /dev/null
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d '{"key":"edgion/prefix/b","value":"val-b"}' > /dev/null
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d '{"key":"edgion/prefix/c","value":"val-c"}' > /dev/null

# GET prefix
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get-prefix/edgion%2Fprefix%2F")
assert_json_success "GET prefix succeeds" "$RESP"
PREFIX_COUNT=$(echo "$RESP" | jq -r '.data.count // 0')
assert_eq "GET prefix returns 3 entries" "3" "$PREFIX_COUNT"

# Verify individual keys in prefix results
assert_contains "Prefix result has key 'a'" "$RESP" "val-a"
assert_contains "Prefix result has key 'b'" "$RESP" "val-b"
assert_contains "Prefix result has key 'c'" "$RESP" "val-c"

# DELETE prefix
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/delete-prefix/edgion%2Fprefix%2F")
assert_json_success "DELETE prefix succeeds" "$RESP"
DELETED_COUNT=$(echo "$RESP" | jq -r '.data.deleted // 0')
assert_eq "DELETE prefix removed 3 keys" "3" "$DELETED_COUNT"

# Verify prefix is empty
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get-prefix/edgion%2Fprefix%2F")
assert_json_success "GET prefix after delete" "$RESP"
assert_eq "Prefix is now empty" "0" "$(echo "$RESP" | jq -r '.data.count // -1')"

# ── 7. Lease Operations ─────────────────────────────────────────────────────
log_info "─── 7. Lease Operations ───"

# Grant a lease (10 second TTL)
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/lease-grant" \
    -d '{"ttl_seconds":10}')
assert_json_success "Lease grant succeeds" "$RESP"
LEASE_ID=$(echo "$RESP" | jq -r '.data.lease_id // 0')
assert_ne "Lease ID is non-zero" "0" "$LEASE_ID"

# PUT with lease
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d "{\"key\":\"edgion/lease-test\",\"value\":\"leased-value\",\"lease_id\":${LEASE_ID}}")
assert_json_success "PUT with lease" "$RESP"

# Verify key exists
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Flease-test")
assert_eq "Leased key value" "leased-value" "$(echo "$RESP" | jq -r '.data.value // ""')"

# Check lease TTL
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/lease-ttl/${LEASE_ID}")
assert_json_success "Lease TTL query succeeds" "$RESP"
TTL=$(echo "$RESP" | jq -r '.data.ttl // -1')
assert_gt "Lease TTL is positive (${TTL}s)" "$TTL" 0

# Revoke lease (should delete attached key)
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/lease-revoke/${LEASE_ID}")
assert_json_success "Lease revoke succeeds" "$RESP"

# Verify key is deleted after lease revoke
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Flease-test")
assert_json_success "GET after lease revoke" "$RESP"
assert_eq "Key deleted after lease revoke" "null" "$(echo "$RESP" | jq -r '.data.value')"

# ── 8. Lease Auto-Expire ────────────────────────────────────────────────────
log_info "─── 8. Lease Auto-Expire ───"

# Grant a short lease (2 second TTL)
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/lease-grant" \
    -d '{"ttl_seconds":2}')
assert_json_success "Short lease grant (2s TTL)" "$RESP"
SHORT_LEASE_ID=$(echo "$RESP" | jq -r '.data.lease_id // 0')
assert_ne "Short lease ID is non-zero" "0" "$SHORT_LEASE_ID"

# PUT with short lease
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d "{\"key\":\"edgion/expire-test\",\"value\":\"will-expire\",\"lease_id\":${SHORT_LEASE_ID}}")
assert_json_success "PUT with short lease" "$RESP"

# Verify key exists immediately
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Fexpire-test")
assert_eq "Key exists before expiry" "will-expire" "$(echo "$RESP" | jq -r '.data.value // ""')"

# Wait for lease to expire
log_info "Waiting 4s for lease to expire..."
sleep 4

# Verify key is auto-deleted
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Fexpire-test")
assert_json_success "GET after lease expiry" "$RESP"
assert_eq "Key auto-deleted after lease expiry" "null" "$(echo "$RESP" | jq -r '.data.value')"

# ── 9. Distributed Lock ───────────────────────────────────────────────────
log_info "─── 9. Distributed Lock ───"

RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/lock" \
    -d '{"name":"edgion/test/lock","ttl_seconds":10,"timeout_seconds":5}')
assert_json_success "Lock acquire + release" "$RESP"
assert_eq "Lock was acquired" "true" "$(echo "$RESP" | jq -r '.data.acquired // false')"
assert_eq "Lock was released" "true" "$(echo "$RESP" | jq -r '.data.released // false')"

# Second lock on different name
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/lock" \
    -d '{"name":"edgion/test/lock2","ttl_seconds":5,"timeout_seconds":3}')
assert_json_success "Lock acquire + release (different name)" "$RESP"
assert_eq "Second lock acquired" "true" "$(echo "$RESP" | jq -r '.data.acquired // false')"

# ── 10. Namespace Isolation ────────────────────────────────────────────────
log_info "─── 10. Namespace Isolation ───"

# Write via namespaced client — key will be auto-prefixed with "/edgion-test/"
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/put" \
    -d '{"key":"ns-key-1","value":"ns-value-1"}')
assert_json_success "NS client PUT key" "$RESP"

# Read via namespaced client — should return the value
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/get/ns-key-1")
assert_json_success "NS client GET key" "$RESP"
assert_eq "NS client GET value" "ns-value-1" "$(echo "$RESP" | jq -r '.data.value // ""')"

# Read via plain client with full prefix — should see the key under the namespace prefix
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/%2Fedgion-test%2Fns-key-1")
assert_json_success "Plain client GET namespaced key (full path)" "$RESP"
assert_eq "Plain client sees NS key" "ns-value-1" "$(echo "$RESP" | jq -r '.data.value // ""')"

# Read via plain client WITHOUT prefix — should NOT see the key
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/ns-key-1")
assert_json_success "Plain client GET without prefix" "$RESP"
assert_eq "Plain client does NOT see key without prefix" "null" "$(echo "$RESP" | jq -r '.data.value')"

# Write via plain client — should NOT be visible to namespaced client
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d '{"key":"plain-only-key","value":"plain-only-value"}')
assert_json_success "Plain client PUT" "$RESP"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/get/plain-only-key")
assert_json_success "NS client GET plain key" "$RESP"
assert_eq "NS client does NOT see plain key" "null" "$(echo "$RESP" | jq -r '.data.value')"

# Prefix query via namespaced client
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/put" \
    -d '{"key":"ns-prefix/a","value":"npa"}' > /dev/null
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/put" \
    -d '{"key":"ns-prefix/b","value":"npb"}' > /dev/null

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/get-prefix/ns-prefix%2F")
assert_json_success "NS client GET prefix" "$RESP"
assert_eq "NS client prefix returns 2 entries" "2" "$(echo "$RESP" | jq -r '.data.count // 0')"

# Cleanup namespace test keys
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/delete-prefix/ns-key" > /dev/null
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_NS_CLIENT}/delete-prefix/ns-prefix" > /dev/null
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/delete/plain-only-key" > /dev/null

# ── 11. Error Handling ─────────────────────────────────────────────────────
log_info "─── 11. Error Handling ───"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/nons_noname/ping")
assert_json_failure "PING non-existent client returns error" "$RESP"
assert_contains "Error mentions 'not found'" "$RESP" "not found"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/nons_noname/health")
assert_json_failure "Health non-existent client returns error" "$RESP"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/nons_noname/get/anykey")
assert_json_failure "GET on non-existent client returns error" "$RESP"

# ── 12. Lifecycle: delete → client removed ─────────────────────────────────
log_info "─── 12. Lifecycle: Delete LinkSys ───"

"$EDGION_CTL" --server "$CONTROLLER_URL" delete LinkSys etcd-test -n default 2>&1 || true
# Wait for sync: Controller → Gateway → ConfHandler → remove client
sleep 5

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/clients")
CLIENTS_JSON=$(echo "$RESP" | jq -r '.data // []')
assert_not_contains "etcd-test removed after delete" "$CLIENTS_JSON" "default/etcd-test"
# namespaced client should still be there
assert_contains "etcd-ns-test still present" "$CLIENTS_JSON" "default/etcd-ns-test"

# ── 13. Lifecycle: re-create → client restored ─────────────────────────────
log_info "─── 13. Lifecycle: Re-create LinkSys ───"

"$EDGION_CTL" --server "$CONTROLLER_URL" apply \
    -f "${CONF_DIR}/LinkSys/Etcd/01_LinkSys_default_etcd-test.yaml" 2>&1 || true
sleep 5

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/clients")
assert_contains "etcd-test restored after re-create" "$RESP" "default/etcd-test"

# Verify re-created client is healthy
RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/ping")
assert_json_success "Re-created client PING succeeds" "$RESP"

# Verify re-created client works (PUT+GET)
RESP=$(gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/put" \
    -d '{"key":"edgion/recreate-test","value":"alive"}')
assert_json_success "Re-created client PUT works" "$RESP"

RESP=$(gw_api GET "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/get/edgion%2Frecreate-test")
assert_eq "Re-created client GET works" "alive" "$(echo "$RESP" | jq -r '.data.value // ""')"

# Cleanup
gw_api POST "/api/v1/testing/link-sys/etcd/${ETCD_CLIENT}/delete/edgion%2Frecreate-test" > /dev/null

# Also delete the namespaced CRD for clean cleanup
"$EDGION_CTL" --server "$CONTROLLER_URL" delete LinkSys etcd-ns-test -n default 2>&1 || true

# =============================================================================
# ──────────────────────────── REPORT ────────────────────────────────────────
# =============================================================================
log_section "Test Report"

echo -e "  Total:   ${TESTS_TOTAL}"
echo -e "  Passed:  ${GREEN}${TESTS_PASSED}${NC}"
echo -e "  Failed:  ${RED}${TESTS_FAILED}${NC}"
echo ""

cat > "${WORK_DIR}/report.log" <<EOF
Etcd LinkSys Integration Test Report
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
