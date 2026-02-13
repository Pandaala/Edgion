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
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

ES_DIR="${PROJECT_ROOT}/examples/test/conf/Services/elasticsearch"
CONF_DIR="${PROJECT_ROOT}/examples/test/conf"
EDGION_CTL="${PROJECT_ROOT}/target/debug/edgion-ctl"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
GATEWAY_BIN="${PROJECT_ROOT}/target/debug/edgion-gateway"
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.toml"
GATEWAY_CONFIG="${PROJECT_ROOT}/config/edgion-gateway.toml"

# ── Ports ───────────────────────────────────────────────────────────────────
CONTROLLER_ADMIN_PORT=5800
GATEWAY_ADMIN_PORT=5900
ES_PORT=19200

CONTROLLER_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
GATEWAY_URL="http://127.0.0.1:${GATEWAY_ADMIN_PORT}"
ES_URL="http://127.0.0.1:${ES_PORT}"

# ── Working directory ───────────────────────────────────────────────────────
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/es_testing_${TIMESTAMP}"
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

        pkill -f edgion-gateway 2>/dev/null || true
        pkill -f edgion-controller 2>/dev/null || true

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

if [ -d "${PROJECT_ROOT}/config/crd" ]; then
    cp -r "${PROJECT_ROOT}/config/crd" "$CONFIG_DIR/"
    log_success "CRD schemas copied"
fi

# =============================================================================
# Step 3: Start Elasticsearch (Docker)
# =============================================================================
log_section "Starting Elasticsearch"
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
# Step 5: Load configuration
# =============================================================================
log_section "Loading configuration"

for f in "${CONF_DIR}/base"/*.yaml; do
    "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$f" > /dev/null 2>&1 || true
done
log_success "Base config loaded"

for f in $(ls "${CONF_DIR}/LinkSys/Elasticsearch"/*.yaml | sort); do
    "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$f" 2>&1
done
log_success "LinkSys/Elasticsearch config loaded"

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

"$EDGION_CTL" --server "$CONTROLLER_URL" delete LinkSys es-test -n default 2>&1 || true
sleep 5

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/clients")
CLIENTS_JSON=$(echo "$RESP" | jq -r '.data // []')
assert_not_contains "es-test removed after delete" "$CLIENTS_JSON" "default/es-test"

# ── 10. Lifecycle: Re-create ──────────────────────────────────────────
log_info "─── 10. Lifecycle: Re-create LinkSys ───"

"$EDGION_CTL" --server "$CONTROLLER_URL" apply \
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
