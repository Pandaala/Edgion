#!/bin/bash
# =============================================================================
# Redis LinkSys Integration Test Script
#
# Standalone test for the Redis LinkSys runtime client.
# Starts a Docker Redis instance, boots Edgion (Controller + Gateway), loads
# a Redis-type LinkSys CRD, and validates the full lifecycle through the
# Gateway Admin API testing endpoints.
#
# Test categories:
#   1. Resource sync          — CRD reaches Gateway
#   2. Client registration    — RedisLinkClient created
#   3. Health & PING          — connectivity + latency
#   4. KV operations          — SET / GET / DEL / INCR
#   5. Hash operations        — HSET / HGET / HGETALL
#   6. List operations        — RPUSH / LPOP / LLEN
#   7. Distributed lock       — acquire + release
#   8. Error handling         — non-existent client
#   9. RateLimitRedis plugin  — allow/deny/headers/key isolation/failure policy
#  10. Lifecycle              — delete CRD → client removed
#
# Usage:
#   ./run_redis_test.sh                  # Full run (build + test + cleanup)
#   ./run_redis_test.sh --no-cleanup     # Keep services alive after test
#   ./run_redis_test.sh --no-build       # Skip cargo build
#   ./run_redis_test.sh --keep-alive     # Alias for --no-cleanup
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

REDIS_DIR="${PROJECT_ROOT}/examples/test/conf/Services/redis"
CONF_DIR="${PROJECT_ROOT}/examples/test/conf"
EDGION_CTL="${PROJECT_ROOT}/target/debug/edgion-ctl"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
GATEWAY_BIN="${PROJECT_ROOT}/target/debug/edgion-gateway"
TEST_SERVER_BIN="${PROJECT_ROOT}/target/debug/examples/test_server"
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.toml"
GATEWAY_CONFIG="${PROJECT_ROOT}/config/edgion-gateway.toml"

# ── Ports ───────────────────────────────────────────────────────────────────
CONTROLLER_ADMIN_PORT=5800
GATEWAY_ADMIN_PORT=5900
REDIS_PORT=16379
TEST_SERVER_HTTP_PORT=30001
PLUGIN_GATEWAY_PORT=31180

CONTROLLER_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
GATEWAY_URL="http://127.0.0.1:${GATEWAY_ADMIN_PORT}"
TEST_SERVER_URL="http://127.0.0.1:${TEST_SERVER_HTTP_PORT}"

# ── Working directory (timestamped, same pattern as start_all_with_conf.sh) ─
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/redis_testing_${TIMESTAMP}"
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

# HTTP response scratch vars (set by gateway_http_request)
HTTP_STATUS=""
HTTP_BODY=""
HTTP_HEADERS=""

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

assert_non_empty() {
    local test_name="$1" value="$2"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if [ -n "$value" ]; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (value is empty)"
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

# Redis client name in URL path: "namespace_name" (underscore-separated)
REDIS_CLIENT="default_redis-test"
RATE_LIMIT_REDIS_HOST="rate-limit-redis.example.com"

# Helper: call plugin gateway listener and capture status/headers/body
gateway_http_request() {
    local path="$1"
    shift

    local header_tmp body_tmp
    header_tmp="$(mktemp)"
    body_tmp="$(mktemp)"

    HTTP_STATUS=$(curl -sS -o "$body_tmp" -D "$header_tmp" -w "%{http_code}" \
        "http://127.0.0.1:${PLUGIN_GATEWAY_PORT}${path}" \
        -H "Host: ${RATE_LIMIT_REDIS_HOST}" \
        -H "Connection: close" "$@" || echo "000")
    HTTP_BODY="$(cat "$body_tmp")"
    HTTP_HEADERS="$(cat "$header_tmp")"

    rm -f "$header_tmp" "$body_tmp"
}

# Helper: get one response header value from the latest gateway_http_request
get_resp_header() {
    local header_name_lc
    header_name_lc="$(echo "$1" | tr '[:upper:]' '[:lower:]')"
    echo "$HTTP_HEADERS" | awk -F': ' -v h="$header_name_lc" '
        {
            key = tolower($1);
            if (key == h) {
                gsub("\r", "", $2);
                print $2;
                exit;
            }
        }'
}

# =============================================================================
# Cleanup (trap on EXIT, same pattern as run_acme_test.sh / kill_all.sh)
# =============================================================================
cleanup() {
    if [ "$DO_CLEANUP" = true ]; then
        log_section "Cleanup"

        # Kill Edgion processes by PID file
        for svc in test_server gateway controller; do
            if [ -f "${PID_DIR}/${svc}.pid" ]; then
                kill "$(cat "${PID_DIR}/${svc}.pid")" 2>/dev/null || true
            fi
        done

        # Fallback: kill by pattern (same as kill_all.sh)
        pkill -f test_server 2>/dev/null || true
        pkill -f edgion-gateway 2>/dev/null || true
        pkill -f edgion-controller 2>/dev/null || true

        # Stop Redis
        log_info "Stopping Redis container..."
        cd "$REDIS_DIR" && docker compose down --timeout 5 2>/dev/null || true

        log_info "Cleanup done"
    else
        log_warn "Services still running (--no-cleanup). Stop with:"
        log_warn "  kill \$(cat ${PID_DIR}/controller.pid) \$(cat ${PID_DIR}/gateway.pid)"
        log_warn "  cd $REDIS_DIR && docker compose down"
    fi
}
trap cleanup EXIT

# =============================================================================
# Step 0: Kill lingering processes (same as start_all_with_conf.sh Step 1)
# =============================================================================
log_section "Cleaning up old processes"
pkill -9 -f test_server 2>/dev/null || true
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
    cargo build --bin edgion-controller --bin edgion-gateway --bin edgion-ctl --example test_server 2>&1 | tail -5
    log_success "Build complete"
else
    log_info "Skipping build (--no-build)"
fi

for bin in "$CONTROLLER_BIN" "$GATEWAY_BIN" "$EDGION_CTL" "$TEST_SERVER_BIN"; do
    if [ ! -f "$bin" ]; then
        log_error "Binary not found: $bin — run without --no-build first"
        exit 1
    fi
done

# =============================================================================
# Step 2: Create work directory (same as start_all_with_conf.sh Step 3)
# =============================================================================
log_section "Preparing work directory"
mkdir -p "$LOG_DIR" "$PID_DIR" "$CONFIG_DIR"

# Copy CRD schemas
if [ -d "${PROJECT_ROOT}/config/crd" ]; then
    cp -r "${PROJECT_ROOT}/config/crd" "$CONFIG_DIR/"
    log_success "CRD schemas copied"
fi

# =============================================================================
# Step 3: Start Redis (Docker)
# =============================================================================
log_section "Starting Redis"
cd "$REDIS_DIR"
docker compose pull --quiet 2>/dev/null || true
docker compose up -d 2>&1 | grep -v "^$" || true

log_info "Waiting for Redis to be healthy..."
for i in $(seq 1 30); do
    if docker exec edgion-test-redis redis-cli -a edgion-test-pwd ping 2>/dev/null | grep -q PONG; then
        log_success "Redis is ready (waited ${i}s)"
        break
    fi
    if [ "$i" -eq 30 ]; then
        log_error "Redis failed to start within 30s"
        docker compose logs
        exit 1
    fi
    sleep 1
done

# =============================================================================
# Step 4: Start test_server backend
# =============================================================================
log_section "Starting test_server"
cd "$PROJECT_ROOT"

"$TEST_SERVER_BIN" \
    --http-ports "${TEST_SERVER_HTTP_PORT}" \
    --grpc-ports "30021" \
    --websocket-port 30005 \
    --tcp-port 30010 \
    --udp-port 30011 \
    --log-level info \
    > "${LOG_DIR}/test_server.log" 2>&1 &
echo $! > "${PID_DIR}/test_server.pid"
log_info "test_server PID: $(cat "${PID_DIR}/test_server.pid")"

for i in $(seq 1 30); do
    if curl -sf "${TEST_SERVER_URL}/health" > /dev/null 2>&1; then
        log_success "test_server ready (waited ${i}s)"
        break
    fi
    if [ "$i" -eq 30 ]; then
        log_error "test_server failed to start"
        tail -30 "${LOG_DIR}/test_server.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
done

# =============================================================================
# Step 5: Start Controller (same args as start_all_with_conf.sh)
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
# Step 6: Load base config + LinkSys/Redis + RateLimitRedis config
# =============================================================================
log_section "Loading configuration"

# Base config (GatewayClass, EdgionGatewayConfig, etc.)
for f in "${CONF_DIR}/base"/*.yaml; do
    "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$f" > /dev/null 2>&1 || true
done
log_success "Base config loaded"

# LinkSys/Redis config (Gateway resource + LinkSys CRD — sorted, so 00_Gateway loads first)
for f in $(ls "${CONF_DIR}/LinkSys/Redis"/*.yaml | sort); do
    "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$f" 2>&1
done
log_success "LinkSys/Redis config loaded"

# Shared plugin Gateway + backend Service/EndpointSlice
"$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "${CONF_DIR}/EdgionPlugins/base/Gateway.yaml" > /dev/null 2>&1
"$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "${CONF_DIR}/HTTPRoute/Basic/Service_test-http.yaml" > /dev/null 2>&1
"$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "${CONF_DIR}/HTTPRoute/Basic/EndpointSlice_test-http.yaml" > /dev/null 2>&1
log_success "Shared EdgionPlugins gateway/backend config loaded"

# RateLimitRedis plugin resources
for f in $(ls "${CONF_DIR}/EdgionPlugins/RateLimitRedis"/*.yaml | sort); do
    "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$f" 2>&1
done
log_success "RateLimitRedis plugin config loaded"

sleep 1

# =============================================================================
# Step 7: Start Gateway (same args as start_all_with_conf.sh)
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

# Wait for ready (same as start_all_with_conf.sh)
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

# Wait for LB preload (same as start_all_with_conf.sh)
for i in $(seq 1 15); do
    if grep -q "LB preload completed" "${LOG_DIR}/gateway.log" 2>/dev/null; then
        log_info "LB preload verified (waited ${i}s)"
        break
    fi
    sleep 1
done

# Give LinkSys ConfHandler time to initialise the Redis client
sleep 2

# =============================================================================
# ──────────────────────────── TEST SUITE ────────────────────────────────────
# =============================================================================

log_section "Test Suite: Redis LinkSys Integration"

# ── 1. Resource sync ────────────────────────────────────────────────────────
log_info "─── 1. Resource Sync ───"

RESP=$(gw_api GET "/configclient/LinkSys?namespace=default&name=redis-test")
assert_json_success "LinkSys CRD synced to Gateway" "$RESP"

# ── 2. Client registration ─────────────────────────────────────────────────
log_info "─── 2. Client Registration ───"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/clients")
assert_json_success "Redis clients endpoint reachable" "$RESP"
assert_contains "redis-test client registered" "$RESP" "default/redis-test"

# ── 3. Health & PING ───────────────────────────────────────────────────────
log_info "─── 3. Health & PING ───"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/ping")
assert_json_success "PING succeeds" "$RESP"
LATENCY=$(echo "$RESP" | jq -r '.data.latency_ms // -1')
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if [ "$LATENCY" -ge 0 ] 2>/dev/null; then
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_success "PING latency valid (${LATENCY}ms)"
else
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_error "PING latency invalid: $LATENCY"
fi

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/health")
assert_json_success "Single client health check" "$RESP"
CONNECTED=$(echo "$RESP" | jq -r '.data.connected // false')
assert_eq "Client connected=true" "true" "$CONNECTED"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/health")
assert_json_success "All-clients health check" "$RESP"

# ── 4. KV Operations ──────────────────────────────────────────────────────
log_info "─── 4. KV Operations ───"

# SET
RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/set" \
    -d '{"key":"edgion:test:kv","value":"hello-edgion","ttl_seconds":60}')
assert_json_success "SET key" "$RESP"

# GET
RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/get/edgion:test:kv")
assert_json_success "GET key" "$RESP"
assert_eq "GET value matches" "hello-edgion" "$(echo "$RESP" | jq -r '.data.value // ""')"

# INCR
RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/set" \
    -d '{"key":"edgion:test:counter","value":"0"}')
RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/incr/edgion:test:counter")
assert_json_success "INCR succeeds" "$RESP"
assert_eq "INCR result is 1" "1" "$(echo "$RESP" | jq -r '.data.value // -1')"

RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/incr/edgion:test:counter")
assert_eq "INCR result is 2" "2" "$(echo "$RESP" | jq -r '.data.value // -1')"

# DEL
RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/del" \
    -d '{"keys":["edgion:test:kv","edgion:test:counter"]}')
assert_json_success "DEL keys" "$RESP"
assert_eq "DEL count is 2" "2" "$(echo "$RESP" | jq -r '.data.deleted // 0')"

# GET after DEL → null
RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/get/edgion:test:kv")
assert_json_success "GET after DEL succeeds" "$RESP"
assert_eq "GET after DEL returns null" "null" "$(echo "$RESP" | jq -r '.data.value')"

# ── 5. Hash Operations ────────────────────────────────────────────────────
log_info "─── 5. Hash Operations ───"

RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/hset" \
    -d '{"key":"edgion:test:hash","field":"name","value":"edgion"}')
assert_json_success "HSET field 'name'" "$RESP"

RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/hset" \
    -d '{"key":"edgion:test:hash","field":"version","value":"1.0"}')
assert_json_success "HSET field 'version'" "$RESP"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/hget/edgion:test:hash/name")
assert_json_success "HGET field 'name'" "$RESP"
assert_eq "HGET value matches" "edgion" "$(echo "$RESP" | jq -r '.data.value // ""')"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/hgetall/edgion:test:hash")
assert_json_success "HGETALL succeeds" "$RESP"
assert_eq "HGETALL has 2 fields" "2" "$(echo "$RESP" | jq -r '.data.fields | length')"

# Cleanup
gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/del" \
    -d '{"keys":["edgion:test:hash"]}' > /dev/null

# ── 6. List Operations ────────────────────────────────────────────────────
log_info "─── 6. List Operations ───"

RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/rpush" \
    -d '{"key":"edgion:test:list","values":["a","b","c"]}')
assert_json_success "RPUSH 3 elements" "$RESP"
assert_eq "RPUSH returns length 3" "3" "$(echo "$RESP" | jq -r '.data.length // 0')"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/llen/edgion:test:list")
assert_json_success "LLEN succeeds" "$RESP"
assert_eq "LLEN returns 3" "3" "$(echo "$RESP" | jq -r '.data.length // 0')"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/lpop/edgion:test:list")
assert_json_success "LPOP succeeds" "$RESP"
assert_eq "LPOP returns 'a'" "a" "$(echo "$RESP" | jq -r '.data.values[0] // ""')"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/llen/edgion:test:list")
assert_eq "LLEN after LPOP is 2" "2" "$(echo "$RESP" | jq -r '.data.length // 0')"

# Cleanup
gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/del" \
    -d '{"keys":["edgion:test:list"]}' > /dev/null

# ── 7. Distributed Lock ───────────────────────────────────────────────────
log_info "─── 7. Distributed Lock ───"

RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/lock" \
    -d '{"key":"edgion:test:lock","ttl_seconds":5,"max_wait_seconds":3}')
assert_json_success "Lock acquire + release" "$RESP"
assert_eq "Lock was acquired" "true" "$(echo "$RESP" | jq -r '.data.acquired // false')"
assert_eq "Lock was released" "true" "$(echo "$RESP" | jq -r '.data.released // false')"

# ── 8. Error handling ─────────────────────────────────────────────────────
log_info "─── 8. Error Handling ───"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/nons_noname/ping")
assert_json_failure "PING non-existent client returns error" "$RESP"
assert_contains "Error mentions 'not found'" "$RESP" "not found"

# ── 9. RateLimitRedis plugin integration ───────────────────────────────────
log_info "─── 9. RateLimitRedis Plugin ───"

RESP=$(gw_api GET "/configclient/EdgionPlugins?namespace=edgion-default&name=rate-limit-redis-main")
assert_json_success "RateLimitRedis main plugin synced" "$RESP"

RATE_KEY_MAIN="rlr-main-$(date +%s%N)"
for i in 1 2 3; do
    gateway_http_request "/test/rate-limit-redis/allow/echo?req=${i}" \
        -H "X-Rate-Key: ${RATE_KEY_MAIN}"
    assert_eq "RateLimitRedis allow request ${i} status" "200" "$HTTP_STATUS"
    assert_eq "RateLimitRedis allow request ${i} limit header" "3" "$(get_resp_header "X-RateLimit-Limit")"
    assert_non_empty "RateLimitRedis allow request ${i} remaining header present" "$(get_resp_header "X-RateLimit-Remaining")"
done

gateway_http_request "/test/rate-limit-redis/allow/echo?req=4" \
    -H "X-Rate-Key: ${RATE_KEY_MAIN}"
assert_eq "RateLimitRedis over-limit request returns 429" "429" "$HTTP_STATUS"
assert_contains "RateLimitRedis over-limit body message" "$HTTP_BODY" "Redis rate limit exceeded"
assert_eq "RateLimitRedis over-limit remaining=0" "0" "$(get_resp_header "X-RateLimit-Remaining")"
assert_non_empty "RateLimitRedis over-limit Retry-After present" "$(get_resp_header "Retry-After")"

RATE_KEY_A="rlr-a-$(date +%s%N)"
RATE_KEY_B="rlr-b-$(date +%s%N)"
for i in 1 2 3; do
    gateway_http_request "/test/rate-limit-redis/allow/isolation-a?req=${i}" \
        -H "X-Rate-Key: ${RATE_KEY_A}"
    assert_eq "RateLimitRedis key A request ${i} status" "200" "$HTTP_STATUS"
done
gateway_http_request "/test/rate-limit-redis/allow/isolation-a?req=4" \
    -H "X-Rate-Key: ${RATE_KEY_A}"
assert_eq "RateLimitRedis key A request 4 blocked" "429" "$HTTP_STATUS"
gateway_http_request "/test/rate-limit-redis/allow/isolation-b?req=1" \
    -H "X-Rate-Key: ${RATE_KEY_B}"
assert_eq "RateLimitRedis key B still allowed after key A exhausted" "200" "$HTTP_STATUS"

gateway_http_request "/test/rate-limit-redis/allow/no-key"
assert_eq "RateLimitRedis missing key allow policy" "200" "$HTTP_STATUS"

gateway_http_request "/test/rate-limit-redis/missing-deny/no-key"
assert_eq "RateLimitRedis missing key deny policy" "429" "$HTTP_STATUS"
assert_contains "RateLimitRedis missing key deny body message" "$HTTP_BODY" "Missing key denied"

gateway_http_request "/test/rate-limit-redis/redis-fail-open/check" \
    -H "X-Rate-Key: rlr-fail-open"
assert_eq "RateLimitRedis onRedisFailure=Allow keeps request open" "200" "$HTTP_STATUS"

gateway_http_request "/test/rate-limit-redis/redis-fail-close/check" \
    -H "X-Rate-Key: rlr-fail-close"
assert_eq "RateLimitRedis onRedisFailure=Deny rejects request" "429" "$HTTP_STATUS"
assert_contains "RateLimitRedis fail-close body message" "$HTTP_BODY" "Redis unavailable (deny)"

# ── 10. Lifecycle: delete → client removed ────────────────────────────────
log_info "─── 10. Lifecycle: Delete LinkSys ───"

"$EDGION_CTL" --server "$CONTROLLER_URL" delete LinkSys redis-test -n default 2>&1 || true
# Wait for sync: Controller → Gateway → ConfHandler → remove client
sleep 5

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/clients")
CLIENTS_JSON=$(echo "$RESP" | jq -r '.data // []')
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if echo "$CLIENTS_JSON" | grep -q "default/redis-test"; then
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_error "Client should be removed after LinkSys delete"
else
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_success "Client removed after LinkSys delete"
fi

# Re-create (useful if --no-cleanup for manual debugging)
"$EDGION_CTL" --server "$CONTROLLER_URL" apply \
    -f "${CONF_DIR}/LinkSys/Redis/01_LinkSys_default_redis-test.yaml" > /dev/null 2>&1 || true

# =============================================================================
# ──────────────────────────── REPORT ────────────────────────────────────────
# =============================================================================
log_section "Test Report"

echo -e "  Total:   ${TESTS_TOTAL}"
echo -e "  Passed:  ${GREEN}${TESTS_PASSED}${NC}"
echo -e "  Failed:  ${RED}${TESTS_FAILED}${NC}"
echo ""

cat > "${WORK_DIR}/report.log" <<EOF
Redis LinkSys Integration Test Report
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
