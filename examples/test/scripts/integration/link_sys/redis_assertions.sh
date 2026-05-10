#!/bin/bash
# =============================================================================
# Redis LinkSys assertions (driver invoked by run_integration.sh)
#
# Assumes the main integration runner has already:
#   - started edgion-controller, edgion-gateway, test_server
#   - confirmed Redis is reachable at 127.0.0.1:16379 (handled by the runner's
#     Redis precondition; see run_integration.sh:start_redis_service_if_needed)
#   - loaded the LinkSys/Redis suite (LinkSys CRD + Secret)
#   - loaded EdgionPlugins/base + EdgionPlugins/RateLimitRedis (so the
#     edgion-plugins-gateway listener and rate-limit-redis-* plugins exist)
#
# This script does NOT start any service or load any CRD. It only issues
# Gateway Admin API calls and real proxy traffic against the already-running
# stack, then exits 0/1 based on the assertion result.
#
# Usage:
#   redis_assertions.sh                     # run sections 1-9 (safe to repeat)
#   redis_assertions.sh --include-lifecycle # also run section 10 (deletes the
#                                           # default/redis-test LinkSys; only
#                                           # safe when no later suite needs it)
# =============================================================================

set -uo pipefail

# ── Colours (match run_integration.sh) ──────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# ── Paths / endpoints ───────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "${SCRIPT_DIR}/../../../../.." && pwd)}"

GATEWAY_URL="${EDGION_GATEWAY_ADMIN_URL:-http://127.0.0.1:5900}"
CONTROLLER_URL="${EDGION_CONTROLLER_URL:-http://127.0.0.1:5800}"
PLUGIN_GATEWAY_PORT="${EDGION_PLUGIN_GATEWAY_PORT:-31180}"
EDGION_CLI="${EDGION_CLI:-${PROJECT_ROOT}/target/debug/edgion-cli}"

REDIS_CLIENT="default_redis-test"
RATE_LIMIT_REDIS_HOST="rate-limit-redis.example.com"
LINKSYS_REDIS_YAML="${PROJECT_ROOT}/examples/test/conf/LinkSys/Redis/01_LinkSys_default_redis-test.yaml"

# ── Args ────────────────────────────────────────────────────────────────────
INCLUDE_LIFECYCLE=false
for arg in "$@"; do
    case "$arg" in
        --include-lifecycle) INCLUDE_LIFECYCLE=true ;;
        -h|--help)
            sed -n '2,22p' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        *)
            echo "Unknown arg: $arg" >&2
            exit 2
            ;;
    esac
done

# ── Counters ────────────────────────────────────────────────────────────────
TESTS_TOTAL=0
TESTS_PASSED=0
TESTS_FAILED=0

# ── Logging ─────────────────────────────────────────────────────────────────
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

# ── Assertions ──────────────────────────────────────────────────────────────
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
    # NOTE: cannot use jq's `// true` here because it treats `false` the same as
    # `null` and would return `true`.
    ok=$(echo "$json_resp" | jq -r 'if .success == false then "false" else "true" end' 2>/dev/null)
    if [ "$ok" = "false" ]; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (expected failure but got success)"
    fi
}

# ── HTTP helpers ────────────────────────────────────────────────────────────
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

# Stateful — sets HTTP_STATUS / HTTP_BODY / HTTP_HEADERS for the most recent call.
HTTP_STATUS=""
HTTP_BODY=""
HTTP_HEADERS=""
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

# ── Pre-flight: tools + gateway readiness ──────────────────────────────────
require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        log_error "Required tool not found: $1"
        exit 1
    fi
}
require_tool curl
require_tool jq

# Wait briefly for the Gateway to register the redis-test client. The runner
# loads the LinkSys CRD just before invoking us; ConfHandler initialisation
# can lag the watch event by a second or two on a cold start.
WAIT_BUDGET=20
elapsed=0
while [ $elapsed -lt $WAIT_BUDGET ]; do
    if gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/ping" \
        | jq -e '.success == true' >/dev/null 2>&1; then
        break
    fi
    sleep 1
    elapsed=$((elapsed + 1))
done
if [ $elapsed -ge $WAIT_BUDGET ]; then
    log_warn "Redis client did not become ready within ${WAIT_BUDGET}s — running tests anyway, expect failures"
fi

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
RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/set" \
    -d '{"key":"edgion:test:kv","value":"hello-edgion","ttl_seconds":60}')
assert_json_success "SET key" "$RESP"

RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/get/edgion:test:kv")
assert_json_success "GET key" "$RESP"
assert_eq "GET value matches" "hello-edgion" "$(echo "$RESP" | jq -r '.data.value // ""')"

RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/set" \
    -d '{"key":"edgion:test:counter","value":"0"}')
RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/incr/edgion:test:counter")
assert_json_success "INCR succeeds" "$RESP"
assert_eq "INCR result is 1" "1" "$(echo "$RESP" | jq -r '.data.value // -1')"

RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/incr/edgion:test:counter")
assert_eq "INCR result is 2" "2" "$(echo "$RESP" | jq -r '.data.value // -1')"

RESP=$(gw_api POST "/api/v1/testing/link-sys/redis/${REDIS_CLIENT}/del" \
    -d '{"keys":["edgion:test:kv","edgion:test:counter"]}')
assert_json_success "DEL keys" "$RESP"
assert_eq "DEL count is 2" "2" "$(echo "$RESP" | jq -r '.data.deleted // 0')"

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

# ── 10. Lifecycle: delete LinkSys → client removed (opt-in) ────────────────
if $INCLUDE_LIFECYCLE; then
    log_info "─── 10. Lifecycle: Delete LinkSys ───"
    if [ ! -x "$EDGION_CLI" ]; then
        log_warn "edgion-cli not found at $EDGION_CLI — skipping lifecycle delete"
    else
        "$EDGION_CLI" --server "$CONTROLLER_URL" delete LinkSys redis-test -n default 2>&1 || true
        # Wait for sync: Controller → Gateway → ConfHandler → remove client
        sleep 5

        RESP=$(gw_api GET "/api/v1/testing/link-sys/redis/clients")
        TESTS_TOTAL=$((TESTS_TOTAL + 1))
        if echo "$RESP" | grep -q "default/redis-test"; then
            TESTS_FAILED=$((TESTS_FAILED + 1))
            log_error "Client should be removed after LinkSys delete"
        else
            TESTS_PASSED=$((TESTS_PASSED + 1))
            log_success "Client removed after LinkSys delete"
        fi

        # Re-create so any subsequent suite that reuses redis-test still works.
        if [ -f "$LINKSYS_REDIS_YAML" ]; then
            "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$LINKSYS_REDIS_YAML" \
                > /dev/null 2>&1 || true
            sleep 2
        fi
    fi
else
    log_info "─── 10. Lifecycle: skipped (pass --include-lifecycle to run) ───"
fi

# =============================================================================
# ──────────────────────────── REPORT ────────────────────────────────────────
# =============================================================================
log_section "Test Report"
echo "Total:  $TESTS_TOTAL"
echo -e "Passed: ${GREEN}$TESTS_PASSED${NC}"
if [ $TESTS_FAILED -gt 0 ]; then
    echo -e "Failed: ${RED}$TESTS_FAILED${NC}"
    exit 1
fi
echo -e "Failed: 0"
exit 0
