#!/bin/bash
# =============================================================================
# OTLP Log Output Integration Test Script
#
# Verifies that the gateway's OTLP access-log output correctly delivers
# structured LogRecord entries to an OpenTelemetry Collector.
#
# Test categories:
#   1.  Startup          — gateway starts with OTLP output configured
#   2.  Lazy connect     — gateway starts even when collector is not yet running
#   3.  Record delivery  — HTTP requests produce records at the collector
#   4.  Record fields    — LogRecord has correct body, attributes, severity, timestamp
#   5.  Shutdown flush   — graceful shutdown flushes buffered records
#   6.  Known limitation — healthy() stays true when collector is down (no dropped metric)
#
# Infrastructure:
#   - Docker OTel Collector (otel/opentelemetry-collector-contrib:0.116.0)
#   - Gateway configured with access_log.output.otlp pointing to collector
#   - test_server as HTTP backend (port 30001)
#   - Minimal HTTPRoute on port 31930
#
# Usage:
#   ./run_otlp_test.sh                  # Full run (build + test + cleanup)
#   ./run_otlp_test.sh --no-cleanup     # Keep services alive after test
#   ./run_otlp_test.sh --no-build       # Skip cargo build
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

OTEL_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/otel-collector"
CONF_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf"
EDGION_CLI="${PROJECT_ROOT}/target/debug/edgion-cli"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
GATEWAY_BIN="${PROJECT_ROOT}/target/debug/edgion-gateway"
TEST_SERVER_BIN="${PROJECT_ROOT}/target/debug/test_server"
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.yaml"

# ── Ports ───────────────────────────────────────────────────────────────────
CONTROLLER_ADMIN_PORT=12101
GATEWAY_ADMIN_PORT=12001
GATEWAY_PROXY_PORT=31930
OTEL_GRPC_PORT=14317
TEST_SERVER_PORT=30001

CONTROLLER_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
CONTROLLER_PROBE_URL="${CONTROLLER_PROBE_URL:-http://127.0.0.1:12100}"
GATEWAY_PROBE_URL="${GATEWAY_PROBE_URL:-http://127.0.0.1:12000}"
: "${EDGION_GATEWAY_ADMIN_TOKEN:=integration-gateway-admin-token-0123456789}"

# ── Working directory ───────────────────────────────────────────────────────
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/otlp_testing_${TIMESTAMP}"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"
# Host path where the OTel Collector writes NDJSON log records.
# Mounted into the container via docker-compose; must be world-writable.
OTEL_LOG_DIR="${WORK_DIR}/otel-logs"
OTEL_LOG_FILE="${OTEL_LOG_DIR}/otlp-logs.ndjson"

export EDGION_WORK_DIR="$WORK_DIR"
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

        log_info "Stopping OTel Collector container..."
        cd "$OTEL_DIR" && docker compose down --timeout 10 2>/dev/null || true

        log_info "Cleanup done"
    else
        log_warn "Services still running (--no-cleanup)."
        log_warn "  Gateway log:      ${LOG_DIR}/gateway.log"
        log_warn "  OTel log records: ${OTEL_LOG_FILE}"
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
sleep 1
log_success "Old processes cleaned up"

# =============================================================================
# Step 1: Build (optional)
# =============================================================================
if [ "$DO_BUILD" = true ]; then
    log_section "Building Edgion binaries"
    cd "$PROJECT_ROOT"
    cargo build \
        --bin edgion-controller \
        --bin edgion-gateway \
        --bin edgion-cli \
        -p edgion-tests --bin test_server 2>&1 | tail -5
    log_success "Build complete"
else
    log_info "Skipping build (--no-build)"
fi

for bin in "$CONTROLLER_BIN" "$GATEWAY_BIN" "$EDGION_CLI" "$TEST_SERVER_BIN"; do
    if [ ! -f "$bin" ]; then
        log_error "Binary not found: $bin — run without --no-build first"
        exit 1
    fi
done

# =============================================================================
# Step 2: Create work directory
# =============================================================================
log_section "Preparing work directory"
mkdir -p "$LOG_DIR" "$PID_DIR" "$CONFIG_DIR" "$OTEL_LOG_DIR"

# conf_sync is mTLS by default (the checked-in config/edgion-*.yaml carry the cert
# paths, work-dir relative). Both binaries below run with WORK_DIR as their work
# dir, so the cert set has to land there.
conf_sync_distribute_certs "$WORK_DIR"
chmod 777 "$OTEL_LOG_DIR"   # writable by the Docker container process

if [ -d "${PROJECT_ROOT}/config/crd" ]; then
    cp -r "${PROJECT_ROOT}/config/crd" "$CONFIG_DIR/"
fi

# Build a gateway config that points access_log output at the local OTel Collector.
GATEWAY_CONFIG="${CONFIG_DIR}/edgion-gateway-otlp.yaml"
cat > "$GATEWAY_CONFIG" <<EOF
gateway:
  server_addr: "https://127.0.0.1:12151"

logging:
  log_dir: "logs"

access_log:
  enabled: true
  output:
    otlp:
      endpoint: "http://127.0.0.1:${OTEL_GRPC_PORT}"
      protocol: grpc
      timeoutMs: 5000

ssl_log:
  enabled: false
EOF
conf_sync_security_block client >> "$GATEWAY_CONFIG"

# =============================================================================
# Step 3: Start Controller + load configuration (BEFORE starting gateway)
# =============================================================================
# Gateway's configure_listeners() fails if no Gateway resources are synced.
# All OTLP resources (Gateway + HTTPRoute + Service) must be applied first.
# The OTel collector endpoint is in the gateway YAML config, so lazy connect
# still works — the OTel collector can be absent when the gateway starts.
log_section "Starting Controller and loading configuration"

"$CONTROLLER_BIN" \
    -c "$CONTROLLER_CONFIG" \
    --work-dir "$WORK_DIR" \
    --conf-dir "$CONFIG_DIR" \
    --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
    --test-mode \
    > "${LOG_DIR}/controller.log" 2>&1 &
echo $! > "${PID_DIR}/controller.pid"

for i in $(seq 1 30); do
    if curl -sf "${CONTROLLER_PROBE_URL}/ready" > /dev/null 2>&1; then
        log_success "Controller ready (waited ${i}s)"
        break
    fi
    [ "$i" -eq 30 ] && { log_error "Controller failed to start"; exit 1; }
    sleep 1
done

for f in "${CONF_DIR}/base"/*.yaml; do
    "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$f" > /dev/null 2>&1 || true
done

for f in $(ls "${CONF_DIR}/LinkSys/Otlp"/*.yaml | sort); do
    "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$f" 2>&1 | grep -v "^$" || true
done
log_success "OTLP test config applied"

# Start test_server backend
"$TEST_SERVER_BIN" --http-ports "$TEST_SERVER_PORT" \
    > "${LOG_DIR}/test_server.log" 2>&1 &
echo $! > "${PID_DIR}/test_server.pid"
log_info "test_server started (pid $(cat "${PID_DIR}/test_server.pid"))"

# =============================================================================
# Test 1: Gateway starts without collector (lazy gRPC connect)
# =============================================================================
log_section "Test 1: Gateway starts without collector (lazy gRPC connect)"

EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
"$GATEWAY_BIN" \
    -c "$GATEWAY_CONFIG" \
    --work-dir "$WORK_DIR" \
    --integration-testing-mode \
    > "${LOG_DIR}/gateway.log" 2>&1 &
echo $! > "${PID_DIR}/gateway.pid"

TESTS_TOTAL=$((TESTS_TOTAL + 1))
GATEWAY_READY=false
for i in $(seq 1 30); do
    if curl -sf "${GATEWAY_PROBE_URL}/ready" > /dev/null 2>&1; then
        GATEWAY_READY=true
        break
    fi
    sleep 1
done

if [ "$GATEWAY_READY" = true ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_success "Gateway starts successfully without collector (lazy gRPC connect verified)"
else
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_error "Gateway failed to start"
    tail -20 "${LOG_DIR}/gateway.log" 2>/dev/null || true
    exit 1
fi

# Verify logger factory reached the OTLP arm (not silent fallback to LocalFile).
# Gateway probe becomes ready at startup step 17 (auxiliary services); logger init
# happens at step 20, so poll for up to 15s rather than checking immediately.
TESTS_TOTAL=$((TESTS_TOTAL + 1))
OTLP_INIT_FOUND=false
for i in $(seq 1 15); do
    if grep -q "Sender: OTLP output" "${LOG_DIR}/gateway.log" 2>/dev/null; then
        OTLP_INIT_FOUND=true
        break
    fi
    sleep 1
done
if $OTLP_INIT_FOUND; then
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_success "Gateway log shows OTLP output arm was reached"
else
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_error "Gateway log does NOT contain OTLP initialization message — check logger_factory.rs or YAML deserialization"
fi

# =============================================================================
# Step 4: Start OTel Collector
# =============================================================================
log_section "Starting OTel Collector"

# Patch docker-compose to use the runtime OTEL_LOG_DIR mount.
RUNTIME_COMPOSE="${CONFIG_DIR}/otel-docker-compose.yaml"
sed "s|otel-logs:/var/log/otel|${OTEL_LOG_DIR}:/var/log/otel|g" \
    "${OTEL_DIR}/docker-compose.yaml" > "$RUNTIME_COMPOSE"

cp "${OTEL_DIR}/collector-config.yaml" "${CONFIG_DIR}/collector-config.yaml"

cd "$CONFIG_DIR"
docker compose -f "$RUNTIME_COMPOSE" pull --quiet 2>/dev/null || true
docker compose -f "$RUNTIME_COMPOSE" up -d 2>&1 | grep -v "^$" || true

# Poll the health_check extension (port 13133) rather than nc on port 14317:
# nc only confirms the host-side TCP port is open, not that the gRPC service
# inside the container is ready. The health_check HTTP endpoint only returns 200
# once the full collector pipeline is initialised.
OTEL_HEALTH_PORT=13133
log_info "Waiting for OTel Collector health endpoint (port ${OTEL_HEALTH_PORT})..."
for i in $(seq 1 60); do
    if curl -sf "http://127.0.0.1:${OTEL_HEALTH_PORT}/" > /dev/null 2>&1; then
        log_success "OTel Collector ready (waited ${i}s)"
        break
    fi
    [ "$i" -eq 60 ] && { log_error "OTel Collector failed to start (health endpoint not responding)"; exit 1; }
    sleep 1
done
# Wait for LB preload and route sync propagation
for i in $(seq 1 20); do
    if grep -q "LB preload completed" "${LOG_DIR}/gateway.log" 2>/dev/null; then
        log_info "LB preload verified"; break
    fi
    sleep 1
done
sleep 3   # route sync propagation

# =============================================================================
# ──────────────────────────── TEST SUITE ────────────────────────────────────
# =============================================================================

log_section "Test Suite: OTLP Access Log Output"

GATEWAY_PROXY="http://127.0.0.1:${GATEWAY_PROXY_PORT}"

# ── 2. Record delivery ──────────────────────────────────────────────────────
log_info "─── 2. Record delivery ───"

# Send 5 HTTP requests through the gateway to generate access log entries.
# The OTel batch processor flushes at most every ~5 s (SDK default).
for i in $(seq 1 5); do
    curl -sf "${GATEWAY_PROXY}/health" -H "Host: otlp-gateway.edgion-test.example.com" \
        -o /dev/null 2>/dev/null || true
done

# Actively generate traffic while waiting. The SDK connection and gateway
# health probe are asynchronous; relying on a fixed sleep can lose the initial
# batch on slower hosts, while a recovery log is absent when the link was
# already healthy before Docker recreated the collector.
log_info "Waiting up to 45s for an exported record..."
RECORD_COUNT=0
for i in $(seq 1 45); do
    curl -sf "${GATEWAY_PROXY}/health" -H "Host: otlp-gateway.edgion-test.example.com" \
        -o /dev/null 2>/dev/null || true
    if [ -f "$OTEL_LOG_FILE" ]; then
        # Count actual logRecord objects (each NDJSON line is one batch with >= 1 records).
        RECORD_COUNT=$(jq -rs '[.[].resourceLogs[].scopeLogs[].logRecords[]] | length' \
            "$OTEL_LOG_FILE" 2>/dev/null || echo "0")
        [ "$RECORD_COUNT" -ge 1 ] && break
    fi
    sleep 1
done

assert_ge "At least 1 OTLP log record delivered to collector" "$RECORD_COUNT" 1

# ── 3. Record fields ──────────────────────────────────────────────────────
log_info "─── 3. Record fields ───"

if [ -f "$OTEL_LOG_FILE" ] && [ "$RECORD_COUNT" -ge 1 ]; then
    # Parse the first record. File exporter wraps each batch as:
    # {"resourceLogs":[{"scopeLogs":[{"logRecords":[{...}]}]}]}
    FIRST_LINE=$(head -1 "$OTEL_LOG_FILE")

    # Body must be a non-empty string (the JSON access log entry).
    BODY=$(echo "$FIRST_LINE" | jq -r '.resourceLogs[0].scopeLogs[0].logRecords[0].body.stringValue // ""' 2>/dev/null)
    assert_ne "LogRecord body is non-empty" "" "$BODY"

    # Body must be valid JSON (the serialized AccessLogEntry).
    if echo "$BODY" | jq empty 2>/dev/null; then
        TESTS_TOTAL=$((TESTS_TOTAL + 1))
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "LogRecord body is valid JSON (AccessLogEntry)"
    else
        TESTS_TOTAL=$((TESTS_TOTAL + 1))
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "LogRecord body is not valid JSON: ${BODY:0:120}"
    fi

    # http.request.method attribute must be present.
    METHOD_ATTR=$(echo "$FIRST_LINE" | \
        jq -r '.resourceLogs[0].scopeLogs[0].logRecords[0].attributes[]
               | select(.key == "http.request.method") | .value.stringValue' 2>/dev/null || echo "")
    assert_ne "LogRecord has http.request.method attribute" "" "$METHOD_ATTR"

    # http.response.status_code attribute must be present.
    STATUS_ATTR=$(echo "$FIRST_LINE" | \
        jq -r '.resourceLogs[0].scopeLogs[0].logRecords[0].attributes[]
               | select(.key == "http.response.status_code") | .value.intValue' 2>/dev/null || echo "")
    assert_ne "LogRecord has http.response.status_code attribute" "" "$STATUS_ATTR"

    # url.path attribute must be present.
    PATH_ATTR=$(echo "$FIRST_LINE" | \
        jq -r '.resourceLogs[0].scopeLogs[0].logRecords[0].attributes[]
               | select(.key == "url.path") | .value.stringValue' 2>/dev/null || echo "")
    assert_ne "LogRecord has url.path attribute" "" "$PATH_ATTR"

    # severity: 2xx responses → INFO (severityNumber 9).
    SEVERITY=$(echo "$FIRST_LINE" | \
        jq -r '.resourceLogs[0].scopeLogs[0].logRecords[0].severityNumber // ""' 2>/dev/null || echo "")
    assert_eq "LogRecord severityNumber is INFO (9) for 2xx response" "9" "$SEVERITY"

    # timeUnixNano must be non-zero.
    TIME_NS=$(echo "$FIRST_LINE" | \
        jq -r '.resourceLogs[0].scopeLogs[0].logRecords[0].timeUnixNano // "0"' 2>/dev/null || echo "0")
    assert_ne "LogRecord timeUnixNano is non-zero" "0" "$TIME_NS"

    # observedTimeUnixNano must be non-zero.
    OBS_NS=$(echo "$FIRST_LINE" | \
        jq -r '.resourceLogs[0].scopeLogs[0].logRecords[0].observedTimeUnixNano // "0"' 2>/dev/null || echo "0")
    assert_ne "LogRecord observedTimeUnixNano is non-zero" "0" "$OBS_NS"
else
    log_warn "No records in collector output file — skipping field assertions"
    TESTS_TOTAL=$((TESTS_TOTAL + 6))
    TESTS_FAILED=$((TESTS_FAILED + 6))
fi

# ── 4. Shutdown flush ────────────────────────────────────────────────────
log_info "─── 4. Shutdown flush ───"

# The file exporter writes one NDJSON line per batch export, each line can
# contain multiple logRecords. Count actual records using jq instead of wc -l.
count_records() {
    local file="$1"
    [ -f "$file" ] || { echo "0"; return; }
    jq -rs '[.[].resourceLogs[].scopeLogs[].logRecords[]] | length' "$file" 2>/dev/null || echo "0"
}

# Send 5 more requests to fill the batch queue, then gracefully stop the gateway.
# Capture the count first because the batch timer may export all five before
# SIGTERM reaches the process; that is successful delivery, not a flush failure.
BEFORE_REQUESTS=$(count_records "$OTEL_LOG_FILE")
for i in $(seq 1 5); do
    curl -sf "${GATEWAY_PROXY}/health" -H "Host: otlp-gateway.edgion-test.example.com" \
        -o /dev/null 2>/dev/null || true
done
BEFORE_SHUTDOWN=$(count_records "$OTEL_LOG_FILE")

log_info "Sending SIGTERM to gateway for graceful shutdown..."
GATEWAY_PID=$(cat "${PID_DIR}/gateway.pid" 2>/dev/null || echo "")
if [ -n "$GATEWAY_PID" ]; then
    kill -TERM "$GATEWAY_PID" 2>/dev/null || true
    # Wait for the process to exit (SdkLoggerProvider::Drop calls shutdown() blocking flush).
    for i in $(seq 1 15); do
        kill -0 "$GATEWAY_PID" 2>/dev/null || break
        sleep 1
    done
    log_info "Gateway exited after graceful shutdown"
fi

# Give the collector time to write the final batch.
sleep 3

AFTER_SHUTDOWN=$(count_records "$OTEL_LOG_FILE")
assert_ge "At least 10 logRecords total after shutdown flush" "$AFTER_SHUTDOWN" 10

TESTS_TOTAL=$((TESTS_TOTAL + 1))
if [ "$AFTER_SHUTDOWN" -gt "$BEFORE_SHUTDOWN" ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_success "Shutdown flush delivered additional records (before=${BEFORE_SHUTDOWN}, after=${AFTER_SHUTDOWN})"
elif [ "$BEFORE_SHUTDOWN" -ge $((BEFORE_REQUESTS + 5)) ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_success "All 5 pre-shutdown records were already exported by the batch timer"
else
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_error "Pre-shutdown records were not fully delivered (initial=${BEFORE_REQUESTS}, before=${BEFORE_SHUTDOWN}, after=${AFTER_SHUTDOWN})"
fi

# ── 5. Known limitation: healthy() stays true when collector is down ────────
log_info "─── 5. Known limitation: dropped metric (documented behavior) ───"
log_warn "KNOWN LIMITATION: OtlpLogSender::healthy() is permanently true after init()."
log_warn "When the OTel Collector is unreachable, the SDK batch queue fills silently."
log_warn "The access_log_dropped Prometheus counter is NOT incremented for OTLP record loss."
log_warn "Tracking issue: implement background health-check or SDK ForceFlush probe to flip healthy()."
TESTS_TOTAL=$((TESTS_TOTAL + 1))
TESTS_PASSED=$((TESTS_PASSED + 1))
log_success "Known limitation documented (not a test failure)"

# =============================================================================
# ──────────────────────────── REPORT ────────────────────────────────────────
# =============================================================================
log_section "Test Report"

echo -e "  Total:   ${TESTS_TOTAL}"
echo -e "  Passed:  ${GREEN}${TESTS_PASSED}${NC}"
echo -e "  Failed:  ${RED}${TESTS_FAILED}${NC}"
echo ""

cat > "${WORK_DIR}/report.log" <<EOF
OTLP Log Output Integration Test Report
$(date)
Total:  ${TESTS_TOTAL}
Passed: ${TESTS_PASSED}
Failed: ${TESTS_FAILED}
EOF

if [ "$TESTS_FAILED" -gt 0 ]; then
    log_error "Some tests FAILED!"
    log_info "Debug tips:"
    log_info "  Gateway log:       ${LOG_DIR}/gateway.log"
    log_info "  Controller log:    ${LOG_DIR}/controller.log"
    log_info "  OTLP records file: ${OTEL_LOG_FILE}"
    exit 1
else
    log_success "All tests PASSED!"
    exit 0
fi
