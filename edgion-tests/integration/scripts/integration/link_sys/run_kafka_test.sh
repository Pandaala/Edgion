#!/bin/bash
# =============================================================================
# Kafka Log Output Integration Test Script
#
# Verifies that the gateway's Kafka access-log output correctly delivers
# log records to a Kafka topic via a KafkaLinkSys resource.
#
# NOTE: Unlike OTLP (lazy gRPC connect), rskafka connects eagerly during
# KafkaLinkClient::init(). The Redpanda broker must be running and the topic
# must exist BEFORE the KafkaLinkSys resource is applied.
#
# Test categories:
#   1.  Startup          — gateway starts with kafka access_log config
#   2.  Record delivery  — HTTP requests produce messages in the topic
#   3.  Record body      — message value is a valid JSON access log entry
#   4.  Lifecycle delete — delete KafkaLinkSys → no new messages
#   5.  Lifecycle restore — re-apply → messages resume
#   6.  Broker outage recovery — broker down→up while LinkSys applied; healthy() degrades then recovers (Mode B)
#
# Usage:
#   ./run_kafka_test.sh                 # Full run (build + test + cleanup)
#   ./run_kafka_test.sh --no-cleanup    # Keep services alive after test
#   ./run_kafka_test.sh --no-build      # Skip cargo build
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

KAFKA_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/kafka"
CONF_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf"
EDGION_CLI="${PROJECT_ROOT}/target/debug/edgion-cli"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
GATEWAY_BIN="${PROJECT_ROOT}/target/debug/edgion-gateway"
TEST_SERVER_BIN="${PROJECT_ROOT}/target/debug/test_server"
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.yaml"

# ── Ports ───────────────────────────────────────────────────────────────────
CONTROLLER_ADMIN_PORT=12101
GATEWAY_PROXY_PORT=31931
REDPANDA_PORT=19092
TEST_SERVER_PORT=30001

CONTROLLER_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
CONTROLLER_PROBE_URL="${CONTROLLER_PROBE_URL:-http://127.0.0.1:12100}"
GATEWAY_PROBE_URL="${GATEWAY_PROBE_URL:-http://127.0.0.1:12000}"
GATEWAY_METRICS_URL="http://127.0.0.1:12090/metrics"
: "${EDGION_GATEWAY_ADMIN_TOKEN:=integration-gateway-admin-token-0123456789}"

KAFKA_TOPIC="edgion-access"

# ── Working directory ───────────────────────────────────────────────────────
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/kafka_testing_${TIMESTAMP}"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"

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
        log_error "$test_name (should NOT be '$not_expected')"
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

# Enforce the consumer deadline inside the container: stopping docker exec on
# the host does not stop rpk. The fixture provides /usr/bin/timeout. Also bound
# the host client in case the Docker daemon stops answering.
consume_topic_for() {
    local wait_secs="$1" max_records="${2:-9999}" bg_pid elapsed=0
    docker exec edgion-test-redpanda \
        /usr/bin/timeout --signal=TERM --kill-after=1 "$wait_secs" \
        rpk topic consume "$KAFKA_TOPIC" -o start --num "$max_records" 2>/dev/null &
    bg_pid=$!
    while kill -0 "$bg_pid" 2>/dev/null && [ "$elapsed" -lt "$((wait_secs + 3))" ]; do
        sleep 1
        elapsed=$((elapsed + 1))
    done
    if kill -0 "$bg_pid" 2>/dev/null; then
        kill -KILL "$bg_pid" 2>/dev/null || true
    fi
    wait "$bg_pid" 2>/dev/null || true
}

# Count messages delivered during a bounded consume from offset=start.
count_topic_messages() {
    local wait_secs="${1:-5}"
    local tmpfile
    tmpfile=$(mktemp)
    consume_topic_for "$wait_secs" > "$tmpfile"
    local count
    count=$(grep '"topic"' "$tmpfile" 2>/dev/null | wc -l | tr -d '[:space:]')
    count="${count:-0}"
    rm -f "$tmpfile"
    echo "$count"
}

# The gauge samples the currently published client at scrape time. Missing or
# unavailable metrics must not be mistaken for an unhealthy client.
kafka_health_is() {
    local expected="$1" metrics
    metrics=$(curl -fsS --connect-timeout 1 --max-time 2 "$GATEWAY_METRICS_URL" 2>/dev/null) || return 1
    printf '%s\n' "$metrics" | awk -v expected="$expected" '
        $1 ~ /^edgion_link_sys_health\{/ &&
        $1 ~ /[{,]link="default\/kafka-test"[,}]/ && $2 == expected { found = 1 }
        END { exit !found }
    '
}

# Match canonical identity, which remains visible under access-log masking.
# A topic count can increase when pre-outage records finish flushing.
topic_has_trace_id() {
    local trace_id="$1"
    consume_topic_for 4 > "${LOG_DIR}/kafka-recovery-records.json"
    jq -se --arg trace_id "$trace_id" '
        any(.[]; (.value | fromjson? | .request_info.trace_id["x-trace-id"]) == $trace_id)
    ' "${LOG_DIR}/kafka-recovery-records.json" >/dev/null 2>&1
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

        pkill -f debug/edgion-gateway    2>/dev/null || true
        pkill -f debug/edgion-controller 2>/dev/null || true
        pkill -f debug/test_server       2>/dev/null || true

        log_info "Stopping Redpanda container..."
        docker compose -f "${KAFKA_DIR}/docker-compose.yaml" down --timeout 10 2>/dev/null || true

        log_info "Cleanup done"
    else
        log_warn "Services still running (--no-cleanup)."
        log_warn "  Consume: docker exec edgion-test-redpanda rpk topic consume ${KAFKA_TOPIC} --num 20 -o start"
    fi
}
trap cleanup EXIT

# =============================================================================
# Step 0: Kill lingering processes
# =============================================================================
log_section "Cleaning up old processes"
pkill -9 -f debug/edgion-gateway    2>/dev/null || true
pkill -9 -f debug/edgion-controller 2>/dev/null || true
pkill -9 -f debug/test_server       2>/dev/null || true
sleep 1

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
        log_error "Binary not found: $bin"
        exit 1
    fi
done

# =============================================================================
# Step 2: Create work directory + gateway config
# =============================================================================
log_section "Preparing work directory"
mkdir -p "$LOG_DIR" "$PID_DIR" "$CONFIG_DIR"

# conf_sync is mTLS by default (the checked-in config/edgion-*.yaml carry the cert
# paths, work-dir relative). Both binaries below run with WORK_DIR as their work
# dir, so the cert set has to land there.
conf_sync_distribute_certs "$WORK_DIR"

if [ -d "${PROJECT_ROOT}/config/crd" ]; then
    cp -r "${PROJECT_ROOT}/config/crd" "$CONFIG_DIR/"
fi

GATEWAY_CONFIG="${CONFIG_DIR}/edgion-gateway-kafka.yaml"
cat > "$GATEWAY_CONFIG" <<EOF
gateway:
  server_addrs: ["https://127.0.0.1:12151"]

logging:
  log_dir: "logs"

access_log:
  enabled: true
  output:
    kafka:
      linkSysRef:
        namespace: "default"
        name: "kafka-test"
      topic: "${KAFKA_TOPIC}"

ssl_log:
  enabled: false
EOF
conf_sync_security_block client >> "$GATEWAY_CONFIG"

# =============================================================================
# Step 3: Start Redpanda and pre-create topic
# =============================================================================
log_section "Starting Redpanda"
docker compose -f "${KAFKA_DIR}/docker-compose.yaml" pull --quiet 2>/dev/null || true
docker compose -f "${KAFKA_DIR}/docker-compose.yaml" up -d 2>&1 | grep -v "^$" || true

log_info "Waiting for Redpanda Kafka port ${REDPANDA_PORT}..."
for i in $(seq 1 60); do
    if nc -z 127.0.0.1 "$REDPANDA_PORT" 2>/dev/null; then
        log_success "Redpanda ready (waited ${i}s)"
        break
    fi
    [ "$i" -eq 60 ] && { log_error "Redpanda failed to start"; exit 1; }
    sleep 1
done

# rskafka uses UnknownTopicHandling::Retry but Redpanda does not auto-create
# topics on produce. Create the topic explicitly before the gateway initialises
# its KafkaLinkClient. Retry: host port may respond before the internal broker
# API is fully initialised.
for i in $(seq 1 15); do
    if docker exec edgion-test-redpanda rpk topic create "$KAFKA_TOPIC" 2>&1 | grep -qE "OK|TOPIC_ALREADY_EXISTS"; then
        log_success "Topic '${KAFKA_TOPIC}' created (waited ${i}s)"; break
    fi
    [ "$i" -eq 15 ] && { log_error "Failed to create Kafka topic"; exit 1; }
    sleep 1
done

# =============================================================================
# Step 4: Start Controller
# =============================================================================
log_section "Starting Controller"

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
        log_success "Controller ready (waited ${i}s)"; break
    fi
    [ "$i" -eq 30 ] && { log_error "Controller failed to start"; exit 1; }
    sleep 1
done

# =============================================================================
# Step 5: Load configuration (BEFORE starting gateway)
# =============================================================================
# The gateway's configure_listeners() fails if no Gateway resources are synced.
# All Kafka configs (Gateway + HTTPRoute + LinkSys) must be applied first so the
# gateway finds them on its initial list. The broker is already running, so the
# LinkSys can be applied here too.
log_section "Loading configuration"

for f in "${CONF_DIR}/base"/*.yaml; do
    "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$f" > /dev/null 2>&1 || true
done

for f in $(ls "${CONF_DIR}/LinkSys/Kafka"/*.yaml | sort); do
    "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$f" 2>&1 | grep -v "^$" || true
done
log_success "Kafka test config applied"

"$TEST_SERVER_BIN" --http-ports "$TEST_SERVER_PORT" \
    > "${LOG_DIR}/test_server.log" 2>&1 &
echo $! > "${PID_DIR}/test_server.pid"

# ── Test 1: Gateway starts with kafka output config ──────────────────────────
log_section "Test 1: Gateway starts with kafka access_log config"

EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
"$GATEWAY_BIN" \
    -c "$GATEWAY_CONFIG" \
    --work-dir "$WORK_DIR" \
    --integration-testing-mode \
    > "${LOG_DIR}/gateway.log" 2>&1 &
echo $! > "${PID_DIR}/gateway.pid"

TESTS_TOTAL=$((TESTS_TOTAL + 1))
for i in $(seq 1 30); do
    if curl -sf "${GATEWAY_PROBE_URL}/ready" > /dev/null 2>&1; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "Gateway starts with kafka access_log output configured"
        break
    fi
    if [ "$i" -eq 30 ]; then
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "Gateway failed to start"
        tail -20 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
done

# KafkaLinkClient::init() runs in a background tokio::spawn. Wait for LB preload
# (confirms routes are ready) then allow extra time for Kafka client init.
for i in $(seq 1 15); do
    if grep -q "All caches are ready" "${LOG_DIR}/gateway.log" 2>/dev/null; then
        log_info "gateway caches ready (waited ${i}s)"; break
    fi
    sleep 1
done
sleep 5

# =============================================================================
# TEST SUITE
# =============================================================================
log_section "Test Suite: Kafka Access Log Output"

GATEWAY_PROXY="http://127.0.0.1:${GATEWAY_PROXY_PORT}"

# ── Test 2: Record delivery ──────────────────────────────────────────────────
log_info "─── 2. Record delivery ───"

for i in $(seq 1 5); do
    curl -sf "${GATEWAY_PROXY}/health" -H "Host: kafka-gateway.edgion-test.example.com" \
        -o /dev/null 2>/dev/null || true
done

# linger_ms=5 means records are flushed within milliseconds.
sleep 2

COUNT_AFTER_SEND=$(count_topic_messages 4)
assert_ge "At least 1 message delivered to Kafka topic" "$COUNT_AFTER_SEND" 1

# ── Test 3: Record body ──────────────────────────────────────────────────────
log_info "─── 3. Record body ───"

if [ "$COUNT_AFTER_SEND" -ge 1 ]; then
    TMPFILE=$(mktemp)
    consume_topic_for 3 1 > "$TMPFILE"
    FIRST_MSG=$(cat "$TMPFILE"); rm -f "$TMPFILE"

    BODY=$(echo "$FIRST_MSG" | jq -r '.value // ""' 2>/dev/null || echo "")
    assert_ne "Message value is non-empty" "" "$BODY"

    if echo "$BODY" | jq empty 2>/dev/null; then
        TESTS_TOTAL=$((TESTS_TOTAL + 1))
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_success "Message body is valid JSON (AccessLogEntry)"
    else
        TESTS_TOTAL=$((TESTS_TOTAL + 1))
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "Message body is not valid JSON: ${BODY:0:120}"
    fi
else
    log_warn "No messages — skipping body assertions"
    TESTS_TOTAL=$((TESTS_TOTAL + 2))
    TESTS_FAILED=$((TESTS_FAILED + 2))
fi

# ── Test 4: Lifecycle — delete KafkaLinkSys ─────────────────────────────────
log_info "─── 4. Lifecycle: delete KafkaLinkSys ───"

"$EDGION_CLI" --server "$CONTROLLER_URL" delete LinkSys kafka-test -n default 2>&1 | grep -v "^$" || true
sleep 3  # wait for shutdown() to flip healthy=false

COUNT_BEFORE_DROP=$(count_topic_messages 4)

for i in $(seq 1 3); do
    curl -sf "${GATEWAY_PROXY}/health" -H "Host: kafka-gateway.edgion-test.example.com" \
        -o /dev/null 2>/dev/null || true
done
sleep 2

COUNT_AFTER_DROP=$(count_topic_messages 4)

TESTS_TOTAL=$((TESTS_TOTAL + 1))
if [ "$COUNT_AFTER_DROP" -le "$COUNT_BEFORE_DROP" ]; then
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_success "No new messages after LinkSys deletion (sender unhealthy)"
else
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_warn "Messages still arriving after deletion (before=${COUNT_BEFORE_DROP}, after=${COUNT_AFTER_DROP})"
fi

# ── Test 5: Lifecycle — re-apply ─────────────────────────────────────────────
log_info "─── 5. Lifecycle: re-apply KafkaLinkSys ───"

"$EDGION_CLI" --server "$CONTROLLER_URL" \
    apply -f "${CONF_DIR}/LinkSys/Kafka/02_LinkSys_default_kafka-test.yaml" \
    2>&1 | grep -v "^$" || true
sleep 5  # wait for init() background task to reconnect

for i in $(seq 1 5); do
    curl -sf "${GATEWAY_PROXY}/health" -H "Host: kafka-gateway.edgion-test.example.com" \
        -o /dev/null 2>/dev/null || true
done
sleep 2

COUNT_AFTER_RESTORE=$(count_topic_messages 4)
assert_ge "Messages resume after re-apply (total > pre-deletion count)" \
    "$COUNT_AFTER_RESTORE" "$((COUNT_AFTER_DROP + 3))"

# ── Test 6: Broker outage recovery (Mode B) ──────────────────────────────────
log_info "─── 6. Broker outage recovery (Mode B) ───"

# -- Down phase: stop the broker without removing the LinkSys resource --------
# Use "stop" (not "down") so the container can be restarted and topic data
# persists. The cleanup() trap uses "down", which handles a stopped-then-started
# container correctly — no change to cleanup() is needed.
log_info "Stopping Redpanda broker (LinkSys resource remains applied)..."
docker compose -f "${KAFKA_DIR}/docker-compose.yaml" stop 2>&1 | grep -v "^$" || true
for i in $(seq 1 5); do
    curl -sf "${GATEWAY_PROXY}/health" -H "Host: kafka-gateway.edgion-test.example.com" \
        -o /dev/null 2>/dev/null || true
done

# Health probing is asynchronous and its interval can exceed a fixed sleep on
# loaded Linux CI hosts. Poll current health; transition logs are rate-limited.
KAFKA_DEGRADED=false
for i in $(seq 1 45); do
    if kafka_health_is 0; then
        KAFKA_DEGRADED=true
        break
    fi
    sleep 1
done
assert_eq "Kafka current health degrades after broker down" "true" "$KAFKA_DEGRADED"

# -- Up phase: restart the broker and wait for readiness ----------------------
log_info "Restarting Redpanda broker..."
docker compose -f "${KAFKA_DIR}/docker-compose.yaml" start 2>&1 | grep -v "^$" || true

log_info "Waiting for Redpanda Kafka port ${REDPANDA_PORT} after restart..."
for i in $(seq 1 60); do
    if nc -z 127.0.0.1 "$REDPANDA_PORT" 2>/dev/null; then
        log_success "Redpanda ready after restart (waited ${i}s)"
        break
    fi
    [ "$i" -eq 60 ] && { log_error "Redpanda failed to come back up after restart"; exit 1; }
    sleep 1
done

# -- Recovery assertion ------------------------------------------------------
# Only send this unique W3C identity after restart, so queued pre-outage records
# cannot satisfy delivery. Core derives canonical x-trace-id from traceparent;
# an incoming x-trace-id alone would be replaced. Paths may be log-masked.
# Require current healthy state AND that exact record; either timeout is a FAIL.
RECOVERY_TRACE_ID=$(printf '%016x%016x' "$(date +%s)" "$$")
RECOVERY_FOUND=false
for attempt in $(seq 1 12); do
    curl -fsS --connect-timeout 1 --max-time 2 "${GATEWAY_PROXY}/health" \
        -H "traceparent: 00-${RECOVERY_TRACE_ID}-0000000000000001-00" \
        -H "Host: kafka-gateway.edgion-test.example.com" -o /dev/null 2>/dev/null || true
    if topic_has_trace_id "$RECOVERY_TRACE_ID" && kafka_health_is 1; then
        RECOVERY_FOUND=true
        break
    fi
    sleep 1
done
assert_eq "Kafka current health and fresh access-log delivery recover after broker restart" "true" "$RECOVERY_FOUND"

# =============================================================================
# REPORT
# =============================================================================
log_section "Test Report"

echo -e "  Total:   ${TESTS_TOTAL}"
echo -e "  Passed:  ${GREEN}${TESTS_PASSED}${NC}"
echo -e "  Failed:  ${RED}${TESTS_FAILED}${NC}"
echo ""

if [ "$TESTS_FAILED" -gt 0 ]; then
    log_error "Some tests FAILED!"
    log_info "  Gateway log:    ${LOG_DIR}/gateway.log"
    log_info "  Controller log: ${LOG_DIR}/controller.log"
    log_info "  Consume topic:  docker exec edgion-test-redpanda rpk topic consume ${KAFKA_TOPIC} -o start --num 20"
    exit 1
else
    log_success "All tests PASSED!"
    exit 0
fi
