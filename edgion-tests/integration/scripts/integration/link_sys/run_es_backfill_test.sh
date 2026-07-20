#!/bin/bash
# =============================================================================
# Elasticsearch Access-Log Backfill Integration Test
#
# End-to-end proof of the access-log "ES outage -> LocalFile fallback ->
# recovery -> manual backfill" flow against a REAL Dockerized Elasticsearch.
#
# Flow under test:
#   1. Boot ES (Docker) + Controller + Gateway, with the access log configured
#      as primary = ES, fallback = bounded LocalFile.
#   2. Generate access-log traffic while ES is DOWN -> entries land in the
#      fallback LocalFile, requests are unaffected, the dropped counter does
#      not spike.
#   3. Bring ES back, wait for the gateway to mark it healthy again.
#   4. POST /api/v1/access-log/backfill -> the fallback file(s) are renamed
#      *.backfilled-<ts>, and the previously-missed docs now appear in ES.
#   5. Re-trigger backfill -> idempotent (no files re-processed).
#
# Usage:
#   ./run_es_backfill_test.sh                  # Full run (build if needed + test + cleanup)
#   ./run_es_backfill_test.sh --no-cleanup     # Keep services alive after test
#   ./run_es_backfill_test.sh --no-build       # Skip cargo build
#   ./run_es_backfill_test.sh --keep-alive     # Alias for --no-cleanup
# =============================================================================

set -euo pipefail

# -- Colours -----------------------------------------------------------------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# -- Paths -------------------------------------------------------------------
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
TEST_SERVER_BIN="${PROJECT_ROOT}/target/debug/test_server"
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.yaml"

# -- Ports -------------------------------------------------------------------
CONTROLLER_ADMIN_PORT=12101
GATEWAY_ADMIN_PORT=12001
GATEWAY_PROXY_PORT=31931
GATEWAY_METRICS_PORT=12090
ES_PORT=19200
TEST_SERVER_PORT=30001

CONTROLLER_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
CONTROLLER_PROBE_URL="${CONTROLLER_PROBE_URL:-http://127.0.0.1:12100}"
GATEWAY_URL="http://127.0.0.1:${GATEWAY_ADMIN_PORT}"
GATEWAY_PROBE_URL="${GATEWAY_PROBE_URL:-http://127.0.0.1:12000}"
GATEWAY_PROXY="http://127.0.0.1:${GATEWAY_PROXY_PORT}"
ES_URL="http://127.0.0.1:${ES_PORT}"
: "${EDGION_GATEWAY_ADMIN_TOKEN:=integration-gateway-admin-token-0123456789}"
# Export so the child gateway process inherits it and configures admin auth.
export EDGION_GATEWAY_ADMIN_TOKEN

# Host used to route proxied traffic through the gateway listener.
ROUTE_HOST="esbackfill-gateway.edgion-test.example.com"
# ES client name in admin URLs (namespace_name).
ES_CLIENT="default_es-test"
# Backfill index family (CRD prefix "edgion-test" + datePattern "%Y.%m.%d").
ES_INDEX_GLOB="edgion-test-*"

# -- Working directory -------------------------------------------------------
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/es_backfill_testing_${TIMESTAMP}"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"

export EDGION_WORK_DIR="$WORK_DIR"
export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
export EDGION_TOKEN="${EDGION_ADMIN_TOKEN}"

# Resolved fallback LocalFile (work_dir-anchored, matches the gateway config).
FALLBACK_FILE="${LOG_DIR}/es_access_fallback.log"

# -- Flags -------------------------------------------------------------------
DO_CLEANUP=true
DO_BUILD=true

# -- Counters ----------------------------------------------------------------
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_TOTAL=0

for arg in "$@"; do
    case $arg in
        --no-cleanup|--keep-alive) DO_CLEANUP=false ;;
        --no-build)                DO_BUILD=false ;;
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
        TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "$test_name (expected='$expected', got='$actual')"
    fi
}

assert_ne() {
    local test_name="$1" not_expected="$2" actual="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if [ "$not_expected" != "$actual" ]; then
        TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "$test_name (should NOT be '$not_expected')"
    fi
}

assert_contains() {
    local test_name="$1" haystack="$2" needle="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if echo "$haystack" | grep -q "$needle"; then
        TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "$test_name (missing '$needle')"
    fi
}

assert_json_success() {
    local test_name="$1" json_resp="$2"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    local ok
    ok=$(echo "$json_resp" | jq -r '.success // false' 2>/dev/null)
    if [ "$ok" = "true" ]; then
        TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "$test_name (success=false, error=$(echo "$json_resp" | jq -r '.error // "unknown"' 2>/dev/null))"
    fi
}

assert_ge() {
    local test_name="$1" actual="$2" threshold="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if [ "$actual" -ge "$threshold" ] 2>/dev/null; then
        TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "$test_name (expected >= $threshold, got $actual)"
    fi
}

assert_le() {
    local test_name="$1" actual="$2" threshold="$3"
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    if [ "$actual" -le "$threshold" ] 2>/dev/null; then
        TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "$test_name"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "$test_name (expected <= $threshold, got $actual)"
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
    if [ -z "$result" ]; then echo '{"success":false,"error":"empty response"}'; else echo "$result"; fi
}

# Helper: cached ES sender health flag as the gateway sees it (drives fallback
# routing + the backfill health gate). /info reports client.healthy().
es_cached_healthy() {
    gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/info" | jq -r '.data.healthy // false' 2>/dev/null
}

# Helper: scrape the dropped-access-log counter from the metrics endpoint.
# Echoes an integer (0 when the metric/endpoint is absent).
dropped_total() {
    local raw
    raw=$(curl -s "http://127.0.0.1:${GATEWAY_METRICS_PORT}/metrics" 2>/dev/null \
        | grep -E '^edgion_access_log_dropped_total' | awk '{print $2}' | head -1)
    if [ -z "$raw" ]; then echo "0"; else printf '%.0f\n' "$raw" 2>/dev/null || echo "0"; fi
}

# Helper: send one proxied request through the gateway (2xx expected).
send_request() {
    curl -s -o /dev/null -H "Host: ${ROUTE_HOST}" "${GATEWAY_PROXY}/health" 2>/dev/null || true
}

# Helper: count docs in the backfill index family (after a refresh).
es_index_count() {
    curl -s -X POST "${ES_URL}/${ES_INDEX_GLOB}/_refresh" >/dev/null 2>&1 || true
    curl -s "${ES_URL}/${ES_INDEX_GLOB}/_count" 2>/dev/null | jq -r '.count // 0' 2>/dev/null || echo "0"
}

# =============================================================================
# Cleanup
# =============================================================================
cleanup() {
    if [ "$DO_CLEANUP" = true ]; then
        log_section "Cleanup"
        for svc in gateway controller test_server; do
            if [ -f "${PID_DIR}/${svc}.pid" ]; then
                kill "$(cat "${PID_DIR}/${svc}.pid")" 2>/dev/null || true
            fi
        done
        pkill -f debug/edgion-gateway 2>/dev/null || true
        pkill -f debug/edgion-controller 2>/dev/null || true
        pkill -f debug/test_server 2>/dev/null || true
        log_info "Stopping Elasticsearch container..."
        cd "$ES_DIR" && docker compose down --timeout 10 2>/dev/null || true
        log_info "Cleanup done"
    else
        log_warn "Services still running (--no-cleanup)."
        log_warn "  Gateway log:    ${LOG_DIR}/gateway.log"
        log_warn "  Fallback file:  ${FALLBACK_FILE}"
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
pkill -9 -f debug/test_server 2>/dev/null || true
sleep 2
log_success "Old processes cleaned up"

# =============================================================================
# Step 1: Build (only if a binary is missing, unless forced off)
# =============================================================================
NEED_BUILD=false
for bin in "$CONTROLLER_BIN" "$GATEWAY_BIN" "$EDGION_CLI" "$TEST_SERVER_BIN"; do
    [ -f "$bin" ] || NEED_BUILD=true
done
if [ "$DO_BUILD" = true ] && [ "$NEED_BUILD" = true ]; then
    log_section "Building Edgion binaries (a binary was missing)"
    cd "$PROJECT_ROOT"
    cargo build --bin edgion-controller --bin edgion-gateway --bin edgion-cli \
        -p edgion-tests --bin test_server 2>&1 | tail -5
    log_success "Build complete"
else
    log_info "Using existing binaries (no build)"
fi
for bin in "$CONTROLLER_BIN" "$GATEWAY_BIN" "$EDGION_CLI" "$TEST_SERVER_BIN"; do
    if [ ! -f "$bin" ]; then log_error "Binary not found: $bin"; exit 1; fi
done

# =============================================================================
# Step 2: Create work directory + custom gateway config
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

# Custom gateway config: access_log primary = ES, fallback = bounded LocalFile.
# NOTE: we deliberately do NOT set EDGION_ACCESS_LOG / EDGION_TEST_ACCESS_LOG_PATH
# so this `output: es` block is what actually takes effect.
GATEWAY_CONFIG="${CONFIG_DIR}/edgion-gateway-es-backfill.yaml"
cat > "$GATEWAY_CONFIG" <<EOF
gateway:
  server_addrs: ["https://127.0.0.1:12151"]

logging:
  log_dir: "logs"
  log_level: "debug,pingora_proxy=error,pingora_core=error"

access_log:
  enabled: true
  output:
    es:
      linkSysRef:
        namespace: default
        name: es-test
  fallback:
    localFile:
      path: logs/es_access_fallback.log
      rotation:
        strategy: { size: 10485760 }   # 10MB
        maxFiles: 3
      diskUsageThresholdPercent: 0      # disable disk shedding in the test

ssl_log:
  enabled: false
EOF
conf_sync_security_block client >> "$GATEWAY_CONFIG"
log_success "Custom gateway config written: ${GATEWAY_CONFIG}"

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
    [ "$i" -eq 90 ] && { log_error "Elasticsearch failed to start within 90s"; docker compose logs; exit 1; }
    sleep 1
done

# =============================================================================
# Step 4: Start Controller + load configuration (BEFORE the gateway)
# =============================================================================
log_section "Starting Controller and loading configuration"
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
        log_success "Controller ready (waited ${i}s)"; break
    fi
    [ "$i" -eq 30 ] && { log_error "Controller failed to start"; tail -30 "${LOG_DIR}/controller.log"; exit 1; }
    sleep 1
done

# Base config (GatewayClass / EdgionGatewayConfig / ReferenceGrant).
for f in "${CONF_DIR}/base"/*.yaml; do
    "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$f" > /dev/null 2>&1 || true
done
log_success "Base config loaded"

# EsBackfill scenario: ES LinkSys (sane maxBodyBytes) + Gateway + HTTPRoute +
# Service + EndpointSlice. The dedicated LinkSys here uses the default 10MB body
# limit so access-log docs are NOT dropped at the bulk layer (unlike the
# LinkSys/Elasticsearch fixture, which sets maxBodyBytes=256 on purpose).
for f in $(ls "${CONF_DIR}/LinkSys/EsBackfill"/*.yaml | sort); do
    # A failed apply of any EsBackfill config (Gateway / HTTPRoute / Service /
    # EndpointSlice / LinkSys) is a HARD error — the whole scenario depends on it.
    if ! "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$f"; then
        log_error "Failed to apply EsBackfill config: ${f}"
        exit 1
    fi
done
log_success "ES + routing config loaded"

# Start test_server backend (always-up 2xx backend).
"$TEST_SERVER_BIN" --http-ports "$TEST_SERVER_PORT" > "${LOG_DIR}/test_server.log" 2>&1 &
echo $! > "${PID_DIR}/test_server.pid"
log_info "test_server started (pid $(cat "${PID_DIR}/test_server.pid"))"
sleep 1

# =============================================================================
# Step 5: Start Gateway
# =============================================================================
log_section "Starting Gateway"
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
        log_success "Gateway ready (waited ${i}s)"; break
    fi
    [ "$i" -eq 60 ] && { log_error "Gateway failed to start"; tail -40 "${LOG_DIR}/gateway.log"; exit 1; }
    sleep 1
done

for i in $(seq 1 20); do
    grep -q "LB preload completed" "${LOG_DIR}/gateway.log" 2>/dev/null && { log_info "LB preload verified"; break; }
    sleep 1
done
# Let the LinkSys ConfHandler initialise the ES client + the access logger.
sleep 6

# =============================================================================
# ──────────────────────────── TEST SUITE ────────────────────────────────────
# =============================================================================
log_section "Test Suite: ES Access-Log Backfill"

# ── 0. Sanity: ES primary + LocalFile fallback registered ───────────────────
log_info "─── 0. Logger wiring + ES client ───"

ES_PRIMARY_FOUND=false
for i in $(seq 1 15); do
    if grep -q "Sender: ES output" "${LOG_DIR}/gateway.log" 2>/dev/null; then ES_PRIMARY_FOUND=true; break; fi
    sleep 1
done
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if $ES_PRIMARY_FOUND; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "Gateway log shows access-log ES primary sender registered"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "Gateway log does NOT show 'Sender: ES output' — access-log primary is not ES"
fi

TESTS_TOTAL=$((TESTS_TOTAL + 1))
if grep -q "Initializing logger with LocalFile output" "${LOG_DIR}/gateway.log" 2>/dev/null \
   && grep -q "es_access_fallback.log" "${LOG_DIR}/gateway.log" 2>/dev/null; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "Gateway log shows LocalFile fallback sender registered"
else
    # Fallback path may not echo the exact filename; accept the generic init line.
    if grep -q "Initializing logger with LocalFile output" "${LOG_DIR}/gateway.log" 2>/dev/null; then
        TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "Gateway log shows a LocalFile sender registered (fallback)"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "No LocalFile fallback init line in gateway log"
    fi
fi

RESP=$(gw_api GET "/api/v1/testing/link-sys/es/${ES_CLIENT}/info")
assert_json_success "ES client (es-test) registered" "$RESP"
assert_eq "ES client healthy at start" "true" "$(echo "$RESP" | jq -r '.data.healthy // false')"

# Sanity: proxied traffic returns 2xx (backend is up).
TESTS_TOTAL=$((TESTS_TOTAL + 1))
CODE=$(curl -s -o /dev/null -w '%{http_code}' -H "Host: ${ROUTE_HOST}" "${GATEWAY_PROXY}/health" 2>/dev/null || echo "000")
if [ "$CODE" = "200" ] || [ "$CODE" = "204" ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "Proxied request returns 2xx (code=$CODE)"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "Proxied request did not return 2xx (code=$CODE)"
fi

# Warm-up: a few requests so at least one ES bulk flush happens while healthy.
for _ in $(seq 1 5); do send_request; done
sleep 3

DROPPED_BASELINE=$(dropped_total)
log_info "Dropped-counter baseline: ${DROPPED_BASELINE}"

# ── 1. Induce ES outage; wait for the gateway to mark ES unhealthy ──────────
log_info "─── 1. ES outage -> unhealthy ───"
cd "$ES_DIR"; docker compose stop elasticsearch >/dev/null 2>&1 || true
cd "$PROJECT_ROOT"
log_info "Elasticsearch stopped; driving traffic until a failed flush downgrades health..."

ES_UNHEALTHY=false
for i in $(seq 1 40); do
    # Keep buffering access-log docs so the 2s bulk flush keeps failing.
    send_request
    if [ "$(es_cached_healthy)" = "false" ]; then
        ES_UNHEALTHY=true
        log_success "ES sender marked unhealthy after ${i}s of outage traffic"
        break
    fi
    sleep 1
done
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if $ES_UNHEALTHY; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "ES cached health flipped to unhealthy"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "ES never flipped unhealthy within 40s"
fi

# ── 2. Outage traffic -> fallback LocalFile; requests unaffected ────────────
log_info "─── 2. Fallback-on-outage ───"

# Truncate any pre-outage boundary lines so we count a clean known burst.
# (The fallback file only exists once the first entry routed to it.)
PRE_BURST_LINES=0
[ -f "$FALLBACK_FILE" ] && PRE_BURST_LINES=$(wc -l < "$FALLBACK_FILE" | tr -d ' ')
log_info "Fallback lines before known burst: ${PRE_BURST_LINES}"

N=20
OK_2XX=0
for _ in $(seq 1 $N); do
    CODE=$(curl -s -o /dev/null -w '%{http_code}' -H "Host: ${ROUTE_HOST}" "${GATEWAY_PROXY}/health" 2>/dev/null || echo "000")
    case "$CODE" in 200|204) OK_2XX=$((OK_2XX + 1)) ;; esac
done
# Give the fallback LocalFile writer queue a moment to flush to disk.
sleep 3

assert_ge "Requests during outage still succeed (2xx >= ${N})" "$OK_2XX" "$N"

TESTS_TOTAL=$((TESTS_TOTAL + 1))
if [ -f "$FALLBACK_FILE" ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "Fallback LocalFile exists: ${FALLBACK_FILE}"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "Fallback LocalFile missing: ${FALLBACK_FILE}"
fi

FALLBACK_LINES=0
[ -f "$FALLBACK_FILE" ] && FALLBACK_LINES=$(wc -l < "$FALLBACK_FILE" | tr -d ' ')
BURST_LINES=$((FALLBACK_LINES - PRE_BURST_LINES))
log_info "Fallback lines after burst: ${FALLBACK_LINES} (delta=${BURST_LINES})"
# Allow a small delta for the outage-boundary batch; expect most of the burst.
assert_ge "Fallback captured the outage burst (>= $((N - 3)) new lines)" "$BURST_LINES" "$((N - 3))"

# Hard guard: if the fallback capture silently failed (BURST_LINES == 0), every
# later assertion phrased as `assert_ge ... $((BURST_LINES - N))` would compare
# against a NEGATIVE threshold and pass vacuously. Refuse to continue with an
# empty burst so the run cannot go green on zero captured lines.
if [ "$BURST_LINES" -lt 1 ] 2>/dev/null; then
    log_error "Fallback captured ZERO burst lines — downstream thresholds would pass vacuously; failing the run"
    TESTS_FAILED=$((TESTS_FAILED + 1))
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    log_info "  Gateway log:    ${LOG_DIR}/gateway.log"
    log_info "  Fallback file:  ${FALLBACK_FILE}"
    exit 1
fi

# No drop spike: the dropped counter must not jump by the burst size.
DROPPED_AFTER=$(dropped_total)
DROPPED_DELTA=$((DROPPED_AFTER - DROPPED_BASELINE))
[ "$DROPPED_DELTA" -lt 0 ] && DROPPED_DELTA=0
log_info "Dropped-counter after outage: ${DROPPED_AFTER} (delta=${DROPPED_DELTA})"
# A handful of boundary entries may be lost in the failing in-flight ES batch,
# but the counter must NOT spike to the burst size — fallback absorbed them.
assert_le "Dropped counter did not spike (delta <= 5)" "$DROPPED_DELTA" 5

# ── 3. Recover ES; wait for the upgrade-only probe to mark it healthy ───────
log_info "─── 3. ES recovery -> healthy ───"
cd "$ES_DIR"; docker compose start elasticsearch >/dev/null 2>&1 || true
cd "$PROJECT_ROOT"

log_info "Waiting for ES cluster health endpoint..."
for i in $(seq 1 60); do
    curl -sf "${ES_URL}/_cluster/health" > /dev/null 2>&1 && { log_success "ES cluster reachable again (waited ${i}s)"; break; }
    [ "$i" -eq 60 ] && { log_error "ES did not come back within 60s"; }
    sleep 1
done

ES_HEALTHY_AGAIN=false
for i in $(seq 1 40); do
    if [ "$(es_cached_healthy)" = "true" ]; then
        ES_HEALTHY_AGAIN=true; log_success "ES sender marked healthy again after ${i}s"; break
    fi
    sleep 1
done
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if $ES_HEALTHY_AGAIN; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "ES cached health recovered to healthy"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "ES never recovered to healthy within 40s"
fi

# Baseline ES doc count just before backfill (includes any pre-outage docs).
COUNT_BEFORE=$(es_index_count)
log_info "ES doc count before backfill: ${COUNT_BEFORE}"

# ── 4. Backfill ─────────────────────────────────────────────────────────────
log_info "─── 4. Manual backfill ───"

BF=$(curl -s -X POST "${GATEWAY_URL}/api/v1/access-log/backfill" \
    -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" \
    -H "Content-Type: application/json" 2>/dev/null)
log_info "Backfill response: ${BF}"

assert_json_success "Backfill returned success" "$BF"
FILES_PROCESSED=$(echo "$BF" | jq -r '.data.summary.files_processed // 0')
LINES_BACKFILLED=$(echo "$BF" | jq -r '.data.summary.lines_backfilled // 0')
LINES_FAILED=$(echo "$BF" | jq -r '.data.summary.lines_failed // 0')
assert_ge "Backfill processed >= 1 file" "$FILES_PROCESSED" 1
assert_ge "Backfill ingested >= $((BURST_LINES - 1)) lines" "$LINES_BACKFILLED" "$((BURST_LINES - 1))"
assert_eq "Backfill reported 0 failed lines" "0" "$LINES_FAILED"

# Fallback file renamed to *.backfilled-<ts>, active file gone.
BACKFILLED_FILES=$(ls "${LOG_DIR}"/es_access_fallback.log*.backfilled-* 2>/dev/null | wc -l | tr -d ' ')
assert_ge "A *.backfilled-<ts> file now exists" "$BACKFILLED_FILES" 1

TESTS_TOTAL=$((TESTS_TOTAL + 1))
if [ ! -f "$FALLBACK_FILE" ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "Active fallback file removed after rename"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "Active fallback file still present after backfill: ${FALLBACK_FILE}"
fi

# Wait for the ES bulk flush of the backfilled docs, then verify count grew.
log_info "Waiting for ES bulk flush of backfilled docs..."
sleep 6
COUNT_AFTER=$(es_index_count)
log_info "ES doc count after backfill: ${COUNT_AFTER}"
COUNT_DELTA=$((COUNT_AFTER - COUNT_BEFORE))
assert_ge "ES doc count increased after backfill (delta >= $((BURST_LINES - 2)))" "$COUNT_DELTA" "$((BURST_LINES - 2))"

# ── 5. Idempotent re-trigger ────────────────────────────────────────────────
log_info "─── 5. Idempotent re-trigger ───"

BF2=$(curl -s -X POST "${GATEWAY_URL}/api/v1/access-log/backfill" \
    -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" \
    -H "Content-Type: application/json" 2>/dev/null)
log_info "Re-trigger response: ${BF2}"
assert_json_success "Re-trigger returned success" "$BF2"
FILES_PROCESSED_2=$(echo "$BF2" | jq -r '.data.summary.files_processed // -1')
LINES_BACKFILLED_2=$(echo "$BF2" | jq -r '.data.summary.lines_backfilled // -1')
assert_eq "Re-trigger processed 0 files (idempotent)" "0" "$FILES_PROCESSED_2"
assert_eq "Re-trigger ingested 0 lines (idempotent)" "0" "$LINES_BACKFILLED_2"

# ES count must not grow on the idempotent re-trigger.
sleep 3
COUNT_FINAL=$(es_index_count)
assert_le "ES doc count unchanged after idempotent re-trigger" "$((COUNT_FINAL - COUNT_AFTER))" 0

# =============================================================================
# ── 6. SECOND outage -> backfill cycle (F1 regression) ──────────────────────
#
# The first backfill renamed the active fallback file to `*.backfilled-<ts>`.
# The LocalFileWriter held an open fd to that inode. The F1 bug: on the NEXT
# outage the writer kept appending into the already-backfilled file (excluded
# from every future backfill), silently losing the entries. The fix reopens a
# fresh `base_path` when it detects the active file was renamed away. This cycle
# proves it end-to-end: the second burst must land in a FRESH es_access_fallback
# .log and its docs must reach ES after a second backfill.
# =============================================================================
log_section "Cycle 2: post-backfill fallback must NOT be lost (F1 regression)"

# Sanity precondition: after cycle-1 backfill the active fallback file is gone
# (renamed to *.backfilled-*). The second burst therefore has to recreate it.
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if [ ! -f "$FALLBACK_FILE" ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "Cycle 2 precondition: active fallback file absent before second outage"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "Cycle 2 precondition broken: active fallback file present before second outage"
fi
BACKFILLED_BEFORE_C2=$(ls "${LOG_DIR}"/es_access_fallback.log*.backfilled-* 2>/dev/null | wc -l | tr -d ' ')
log_info "Backfilled files before cycle 2: ${BACKFILLED_BEFORE_C2}"

# ── 6.1 Induce the SECOND ES outage; wait for cached health to flip ─────────
log_info "─── 6.1 Second ES outage -> unhealthy ───"
cd "$ES_DIR"; docker compose stop elasticsearch >/dev/null 2>&1 || true
cd "$PROJECT_ROOT"
log_info "Elasticsearch stopped again; driving traffic until the failed flush downgrades health..."

ES_UNHEALTHY_2=false
for i in $(seq 1 40); do
    send_request
    if [ "$(es_cached_healthy)" = "false" ]; then
        ES_UNHEALTHY_2=true
        log_success "ES sender marked unhealthy after ${i}s of second-outage traffic"
        break
    fi
    sleep 1
done
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if $ES_UNHEALTHY_2; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "ES cached health flipped to unhealthy (cycle 2)"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "ES never flipped unhealthy within 40s (cycle 2)"
fi

# ── 6.2 Drive a second burst; it MUST recreate a fresh active fallback file ──
log_info "─── 6.2 Second burst -> fresh fallback file ───"
# The active fallback file was renamed away by cycle-1 backfill, so a healthy
# writer reopens base_path and starts at 0 lines.
PRE_BURST_LINES_2=0
[ -f "$FALLBACK_FILE" ] && PRE_BURST_LINES_2=$(wc -l < "$FALLBACK_FILE" | tr -d ' ')
log_info "Fallback lines before second burst: ${PRE_BURST_LINES_2}"

OK_2XX_2=0
for _ in $(seq 1 $N); do
    CODE=$(curl -s -o /dev/null -w '%{http_code}' -H "Host: ${ROUTE_HOST}" "${GATEWAY_PROXY}/health" 2>/dev/null || echo "000")
    case "$CODE" in 200|204) OK_2XX_2=$((OK_2XX_2 + 1)) ;; esac
done
# Give the fallback LocalFile writer queue a moment to flush + reopen base_path.
sleep 4

assert_ge "Second-outage requests still succeed (2xx >= ${N})" "$OK_2XX_2" "$N"

# THE F1 PROOF: the second burst must land in a FRESH active es_access_fallback
# .log. If the writer had not reopened base_path, this file would be missing
# (the burst would have gone into the *.backfilled-* file instead).
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if [ -f "$FALLBACK_FILE" ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "F1: fresh active fallback file recreated after backfill rename"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "F1 REGRESSION: active fallback file missing — second burst was NOT captured in a fresh file"
    log_error "  Fallback dir listing:"; ls -la "${LOG_DIR}"/es_access_fallback.log* 2>&1 | sed 's/^/    /'
    log_error "  Gateway reopen log lines:"; grep -i "reopen" "${LOG_DIR}/gateway.log" 2>&1 | tail -5 | sed 's/^/    /'
fi

FALLBACK_LINES_2=0
[ -f "$FALLBACK_FILE" ] && FALLBACK_LINES_2=$(wc -l < "$FALLBACK_FILE" | tr -d ' ')
BURST_LINES_2=$((FALLBACK_LINES_2 - PRE_BURST_LINES_2))
log_info "Fallback lines after second burst: ${FALLBACK_LINES_2} (delta=${BURST_LINES_2})"
assert_ge "Fresh fallback captured the second burst (>= $((N - 3)) new lines)" "$BURST_LINES_2" "$((N - 3))"

# Hard guard mirroring T3: a vacuous (zero) second burst would let later
# `$((BURST_LINES_2 - N))` thresholds pass negatively.
if [ "$BURST_LINES_2" -lt 1 ] 2>/dev/null; then
    log_error "Second burst captured ZERO fallback lines — F1 fix appears broken; failing the run"
    log_error "  Fallback dir listing:"; ls -la "${LOG_DIR}"/es_access_fallback.log* 2>&1 | sed 's/^/    /'
    log_error "  Gateway reopen log lines:"; grep -i "reopen" "${LOG_DIR}/gateway.log" 2>&1 | tail -5 | sed 's/^/    /'
    TESTS_FAILED=$((TESTS_FAILED + 1)); TESTS_TOTAL=$((TESTS_TOTAL + 1))
    exit 1
fi

# ── 6.3 Recover ES again ────────────────────────────────────────────────────
log_info "─── 6.3 Second ES recovery -> healthy ───"
cd "$ES_DIR"; docker compose start elasticsearch >/dev/null 2>&1 || true
cd "$PROJECT_ROOT"

log_info "Waiting for ES cluster health endpoint (cycle 2)..."
for i in $(seq 1 60); do
    curl -sf "${ES_URL}/_cluster/health" > /dev/null 2>&1 && { log_success "ES cluster reachable again (waited ${i}s)"; break; }
    [ "$i" -eq 60 ] && { log_error "ES did not come back within 60s (cycle 2)"; }
    sleep 1
done

ES_HEALTHY_AGAIN_2=false
for i in $(seq 1 40); do
    if [ "$(es_cached_healthy)" = "true" ]; then
        ES_HEALTHY_AGAIN_2=true; log_success "ES sender marked healthy again after ${i}s (cycle 2)"; break
    fi
    sleep 1
done
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if $ES_HEALTHY_AGAIN_2; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "ES cached health recovered to healthy (cycle 2)"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "ES never recovered to healthy within 40s (cycle 2)"
fi

# ── 6.4 Second backfill: the second-burst docs must reach ES ────────────────
log_info "─── 6.4 Second backfill ───"
COUNT_BEFORE_C2=$(es_index_count)
log_info "ES doc count before second backfill: ${COUNT_BEFORE_C2}"

BF3=$(curl -s -X POST "${GATEWAY_URL}/api/v1/access-log/backfill" \
    -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" \
    -H "Content-Type: application/json" 2>/dev/null)
log_info "Second backfill response: ${BF3}"

assert_json_success "Second backfill returned success" "$BF3"
FILES_PROCESSED_C2=$(echo "$BF3" | jq -r '.data.summary.files_processed // 0')
LINES_BACKFILLED_C2=$(echo "$BF3" | jq -r '.data.summary.lines_backfilled // 0')
LINES_FAILED_C2=$(echo "$BF3" | jq -r '.data.summary.lines_failed // 0')
assert_ge "Second backfill processed >= 1 file" "$FILES_PROCESSED_C2" 1
assert_ge "Second backfill ingested >= $((BURST_LINES_2 - 1)) lines" "$LINES_BACKFILLED_C2" "$((BURST_LINES_2 - 1))"
assert_eq "Second backfill reported 0 failed lines" "0" "$LINES_FAILED_C2"

# A NEW *.backfilled-<ts> file must exist now (>= 2 total, strictly more than
# before cycle 2). The freshly-recreated active file was renamed in turn.
BACKFILLED_AFTER_C2=$(ls "${LOG_DIR}"/es_access_fallback.log*.backfilled-* 2>/dev/null | wc -l | tr -d ' ')
log_info "Backfilled files after cycle 2: ${BACKFILLED_AFTER_C2}"
assert_ge "A NEW *.backfilled-<ts> file exists after cycle 2 (>= 2 total)" "$BACKFILLED_AFTER_C2" 2
assert_ge "Backfilled-file count grew vs before cycle 2" "$BACKFILLED_AFTER_C2" "$((BACKFILLED_BEFORE_C2 + 1))"

# The active file must be gone again after the second rename.
TESTS_TOTAL=$((TESTS_TOTAL + 1))
if [ ! -f "$FALLBACK_FILE" ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1)); log_success "Active fallback file removed after second rename"
else
    TESTS_FAILED=$((TESTS_FAILED + 1)); log_error "Active fallback file still present after second backfill: ${FALLBACK_FILE}"
fi

# The decisive end-to-end check: the second-burst docs actually reach ES.
log_info "Waiting for ES bulk flush of the second backfill..."
sleep 6
COUNT_AFTER_C2=$(es_index_count)
log_info "ES doc count after second backfill: ${COUNT_AFTER_C2}"
COUNT_DELTA_C2=$((COUNT_AFTER_C2 - COUNT_BEFORE_C2))
assert_ge "F1: ES doc count increased after second backfill (delta >= $((BURST_LINES_2 - 2)))" "$COUNT_DELTA_C2" "$((BURST_LINES_2 - 2))"

# =============================================================================
# ──────────────────────────── REPORT ────────────────────────────────────────
# =============================================================================
log_section "Test Report"
echo -e "  Total:   ${TESTS_TOTAL}"
echo -e "  Passed:  ${GREEN}${TESTS_PASSED}${NC}"
echo -e "  Failed:  ${RED}${TESTS_FAILED}${NC}"
echo ""

cat > "${WORK_DIR}/report.log" <<EOF
ES Access-Log Backfill Integration Test Report
$(date)
Total:  ${TESTS_TOTAL}
Passed: ${TESTS_PASSED}
Failed: ${TESTS_FAILED}
EOF

if [ "$TESTS_FAILED" -gt 0 ]; then
    log_error "Some tests FAILED!"
    log_info "  Gateway log:    ${LOG_DIR}/gateway.log"
    log_info "  Controller log: ${LOG_DIR}/controller.log"
    exit 1
else
    log_success "All tests PASSED!"
    exit 0
fi
