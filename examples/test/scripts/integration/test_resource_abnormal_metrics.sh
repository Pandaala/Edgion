#!/bin/bash
# =============================================================================
# Resource Abnormal Metrics Integration Test
#
# Verifies edgion_controller_resource_abnormal gauge end-to-end:
#   1. Seed a bad HTTPRoute whose embedded status has ResolvedRefs:False
#      → expect an "err" series in /metrics within a few snapshot ticks.
#   2. Overwrite with a recovered variant (ResolvedRefs:True) → expect
#      the abnormal series to disappear on the next tick.
#   3. Delete the bad HTTPRoute file → expect no leftover (ghost) series.
#
# Uses the FileSystem backend with tick_secs=30 (the current minimum
# enforced by validate_metrics). No K8s / port-forward / test_client
# binary required. Each phase waits up to TICK_SECS * 3 = 90s for the
# snapshot loop to reflect the expected state, so the whole test is
# ~5-7 minutes worst case.
#
# Note on FileSystem mode: the status-metrics snapshot reads from the
# controller's in-memory LocalRawStore, which is populated from the input
# YAML content. In K8s mode the reflector store naturally carries whatever
# status the controller writes back; in FileSystem mode status lives in a
# sibling `.status` file and does not flow back into the raw store, so the
# fixture embeds the status block directly. See `status_metrics.rs` and
# `raw_store.rs` for details.
# =============================================================================
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
FIXTURE_DIR="${PROJECT_ROOT}/examples/test/conf/Status/ResourceAbnormalMetrics"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/resource_abnormal_metrics_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
CONF_WATCH_DIR="${WORK_DIR}/conf"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"

CONTROLLER_ADMIN_PORT=15810
CONTROLLER_GRPC_PORT=15811
METRICS_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/metrics"
TICK_SECS=30

CONTROLLER_PID=""
TEST_PASSED=0

# --- Log helpers ---
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC}   $1"; }
log_error()   { echo -e "${RED}[ERR]${NC}  $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# --- Cleanup ---
cleanup() {
    log_section "Cleanup"
    if [[ -n "$CONTROLLER_PID" ]] && kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_info "Stopping controller PID=$CONTROLLER_PID"
        kill "$CONTROLLER_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$CONTROLLER_PID" 2>/dev/null || true
    fi
    if [[ "$TEST_PASSED" -eq 1 ]]; then
        log_info "Removing WORK_DIR (test passed): $WORK_DIR"
        rm -rf "$WORK_DIR"
    else
        log_warn "Keeping WORK_DIR for post-mortem: $WORK_DIR"
    fi
}
trap cleanup EXIT

# --- Setup ---
log_section "Setup"
mkdir -p "$CONFIG_DIR" "$CONF_WATCH_DIR" "$LOG_DIR" "$PID_DIR"
log_info "WORK_DIR=$WORK_DIR"

# Make CRD schemas reachable from work_dir (non-K8s mode requires config/crd).
ln -s "${PROJECT_ROOT}/config/crd" "${CONFIG_DIR}/crd"

# Controller config pinned at the minimum tick (30s) so the snapshot loop
# rebuilds as quickly as the validator allows.
cat > "${CONFIG_DIR}/edgion-controller.yaml" <<EOF
work_dir: "${WORK_DIR}"

server:
  grpc_listen: "127.0.0.1:${CONTROLLER_GRPC_PORT}"
  admin_listen: "127.0.0.1:${CONTROLLER_ADMIN_PORT}"

logging:
  log_dir: "${LOG_DIR}"
  log_prefix: edgion-controller
  log_level: info
  json_format: false
  console: false
  buffer_size: 10000

conf_center:
  type: file_system
  conf_dir: "${CONF_WATCH_DIR}"

debug:
  enabled: false

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200

metrics:
  resource_abnormal:
    enabled: true
    tick_secs: ${TICK_SECS}
EOF
log_info "Config file written: ${CONFIG_DIR}/edgion-controller.yaml"

# Pre-seed the watched conf_dir with the bad route + its parent Gateway /
# GatewayClass. All three land in the watched dir before the controller
# boots, so the first snapshot tick (which runs ~TICK_SECS after registrar
# bringup) should already classify the route as abnormal.
cp "${FIXTURE_DIR}/GatewayClass__abn-test-class.yaml"       "${CONF_WATCH_DIR}/"
cp "${FIXTURE_DIR}/Gateway_default_abn-test-gateway.yaml"   "${CONF_WATCH_DIR}/"
cp "${FIXTURE_DIR}/HTTPRoute_default_abn-bad-route.yaml"    "${CONF_WATCH_DIR}/"
log_info "Seeded conf_dir with 3 fixtures (GatewayClass, Gateway, bad HTTPRoute)"

# --- Launch controller ---
log_section "Start Controller"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
if [[ ! -x "$CONTROLLER_BIN" ]]; then
    log_error "Controller binary not found at $CONTROLLER_BIN"
    log_info "Build with: cargo build --bin edgion-controller"
    exit 1
fi

"$CONTROLLER_BIN" \
    --config-file "${CONFIG_DIR}/edgion-controller.yaml" \
    > "${LOG_DIR}/controller.stdout.log" 2> "${LOG_DIR}/controller.stderr.log" &
CONTROLLER_PID=$!
echo "$CONTROLLER_PID" > "${PID_DIR}/controller.pid"
log_info "Controller started PID=$CONTROLLER_PID"

# Wait for /health to answer 200.
log_info "Waiting for controller readiness..."
READY=0
for i in $(seq 1 30); do
    if curl -sf "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/health" > /dev/null 2>&1; then
        READY=1
        break
    fi
    if ! kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_error "Controller process exited during startup"
        log_info "=== controller.stderr.log (last 40 lines) ==="
        tail -40 "${LOG_DIR}/controller.stderr.log" 2>/dev/null || true
        log_info "=== controller.stdout.log (last 40 lines) ==="
        tail -40 "${LOG_DIR}/controller.stdout.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
done
if [[ "$READY" -ne 1 ]]; then
    log_error "Controller did not become ready in 30s"
    tail -40 "${LOG_DIR}/controller.stderr.log" 2>/dev/null || true
    exit 1
fi
log_success "Controller ready on port ${CONTROLLER_ADMIN_PORT}"

# --- Assertion helpers ---
scrape() { curl -sS "$METRICS_URL"; }

# Wait until /metrics contains a line matching a grep regex. Timeout is
# generous (3 ticks) since the first snapshot only runs after all three
# fixtures have been ingested + the resource processor has computed status.
wait_for_series() {
    local pattern="$1"
    local label="$2"
    local timeout=$(( TICK_SECS * 3 + 10 ))
    local elapsed=0
    while (( elapsed < timeout )); do
        if scrape | grep -E -q -- "$pattern"; then
            log_success "$label: series present after ${elapsed}s"
            return 0
        fi
        sleep 2
        elapsed=$(( elapsed + 2 ))
    done
    log_error "$label: TIMEOUT (${timeout}s); series not found"
    log_info "=== current /metrics tail ==="
    scrape | tail -40
    log_info "=== controller.stderr.log tail ==="
    tail -40 "${LOG_DIR}/controller.stderr.log" 2>/dev/null || true
    return 1
}

wait_for_no_series() {
    local pattern="$1"
    local label="$2"
    local timeout=$(( TICK_SECS * 3 + 10 ))
    local elapsed=0
    while (( elapsed < timeout )); do
        if ! scrape | grep -E -q -- "$pattern"; then
            log_success "$label: series absent after ${elapsed}s"
            return 0
        fi
        sleep 2
        elapsed=$(( elapsed + 2 ))
    done
    log_error "$label: TIMEOUT (${timeout}s); series still present"
    log_info "=== current /metrics tail ==="
    scrape | tail -40
    return 1
}

# --- Test sequence ---
log_section "Test 1: abnormal resource → err series appears"
# err label is a JSON array string (escaped per Prom text format); non-empty
# means at least one err-level trigger fired.
wait_for_series \
    'edgion_controller_resource_abnormal\{[^}]*name="abn-bad-route"[^}]*err="\[' \
    "abnormal-appears"

log_section "Test 2: resource recovers → err series disappears"
# Overwrite the HTTPRoute file with a variant whose embedded status
# shows ResolvedRefs:True. The file-watcher dispatches an Apply event,
# the raw store is upserted with the recovered object, and the next
# snapshot tick reclassifies the resource as clean.
cp "${FIXTURE_DIR}/HTTPRoute_default_abn-bad-route.recovered.yaml" \
   "${CONF_WATCH_DIR}/HTTPRoute_default_abn-bad-route.yaml"
log_info "Overwrote HTTPRoute with recovered variant (ResolvedRefs:True)"
wait_for_no_series \
    'edgion_controller_resource_abnormal\{[^}]*name="abn-bad-route"' \
    "abnormal-recovers"

log_section "Test 3: delete bad HTTPRoute → no ghost series"
rm -f "${CONF_WATCH_DIR}/HTTPRoute_default_abn-bad-route.yaml"
log_info "Deleted bad HTTPRoute"
# Give the file-watcher time to observe the delete plus one full snapshot tick.
sleep $(( TICK_SECS + 4 ))
if scrape | grep -E -q 'edgion_controller_resource_abnormal\{[^}]*name="abn-bad-route"'; then
    log_error "Ghost series found after resource deletion"
    scrape | grep -E 'edgion_controller_resource_abnormal' || true
    exit 1
fi
log_success "no-ghost: no leftover series after deletion"

log_section "PASS"
TEST_PASSED=1
exit 0
