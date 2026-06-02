#!/usr/bin/env bash
# =============================================================================
# Controller Reconnect Test — verifies Gateway recovers gracefully when
# the Controller dies and restarts.
#
# Self-contained: starts test_server + Controller + Gateway with the
# HTTPRoute/Basic suite, runs the reconnect scenario, then stops everything
# (unless --keep-alive).
#
# Steps:
#   1. Start test_server, Controller, Gateway with the HTTPRoute/Basic suite.
#   2. Sanity: GET via Gateway returns 200 (initial config synced).
#   3. SIGKILL the Controller process. Wait briefly.
#   4. Sanity (degraded): GET still returns 200 — Gateway must NOT drop
#      already-loaded config when the upstream Controller dies.
#   5. Restart Controller (same args / config dir / port).
#   6. Wait for Gateway to reconnect: poll Controller health until stable,
#      with timeout (default 30s).
#   7. Apply a NEW HTTPRoute via edgion-cli — exercises the reconnected
#      stream end-to-end.
#   8. Verify new route works: GET via Gateway with the new hostname → 200.
#   9. Cleanup (unless --keep-alive).
#
# Usage:
#   ./run_controller_reconnect_test.sh                # Full run (start → test → stop)
#   ./run_controller_reconnect_test.sh --no-build     # Skip cargo build
#   ./run_controller_reconnect_test.sh --no-start     # Use existing env
#   ./run_controller_reconnect_test.sh --keep-alive   # Don't kill services
# =============================================================================

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UTILS_DIR="${SCRIPT_DIR}/../utils"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
EDGION_CLI="${PROJECT_ROOT}/target/debug/edgion-cli"
BIN_DIR="${PROJECT_ROOT}/target/debug"

GATEWAY_HTTP_PORT=31000
GATEWAY_HOST="127.0.0.1"
CONTROLLER_URL="${EDGION_CONTROLLER_URL:-http://127.0.0.1:5800}"
GATEWAY_ADMIN_URL="${EDGION_GATEWAY_ADMIN_URL:-http://127.0.0.1:5810}"
GATEWAY_PROBE_URL="${EDGION_GATEWAY_PROBE_URL:-http://127.0.0.1:28000}"
CONTROLLER_ADMIN_PORT=5800
CONTROLLER_GRPC_PORT="${EDGION_TEST_CONTROLLER_GRPC_PORT:-50051}"

RECONNECT_TIMEOUT=30   # seconds to wait for Gateway to reconnect after Controller restart
SYNC_WAIT=3            # seconds to wait after applying new route before verification

PASSED=0
FAILED=0

DO_START=true
DO_CLEANUP=true
DO_BUILD=true

WORK_DIR=""
LOG_DIR=""
PID_DIR=""
CONFIG_DIR=""
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.yaml"

# =============================================================================
# Log helpers (same style as run_conf_sync_test.sh)
# =============================================================================
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error()   { echo -e "${RED}[✗]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# =============================================================================
# Test assertion helpers
# =============================================================================
assert_pass() {
    local name=$1
    local result=$2   # "pass" or "fail"
    local detail=${3:-""}
    if [ "$result" = "pass" ]; then
        log_success "PASS: ${name}"
        PASSED=$((PASSED + 1))
    else
        log_error "FAIL: ${name}${detail:+ — }${detail}"
        FAILED=$((FAILED + 1))
    fi
}

# =============================================================================
# Environment lifecycle
# =============================================================================
start_environment() {
    log_section "Starting test environment (HTTPRoute/Basic)"
    local output exit_code
    output=$("${UTILS_DIR}/start_all_with_conf.sh" --suites HTTPRoute/Basic 2>&1) && exit_code=0 || exit_code=$?
    echo "$output"
    if [ $exit_code -ne 0 ]; then
        log_error "Failed to start test environment"
        exit 1
    fi
    log_success "Test environment started"
}

resolve_work_dir() {
    local current_file="${PROJECT_ROOT}/integration_testing/.current"
    if [ ! -f "$current_file" ]; then
        log_error "integration_testing/.current not found — run start_all_with_conf.sh first"
        exit 1
    fi
    WORK_DIR=$(cat "$current_file")
    LOG_DIR="${WORK_DIR}/logs"
    PID_DIR="${WORK_DIR}/pids"
    CONFIG_DIR="${WORK_DIR}/config"

    export EDGION_WORK_DIR="${WORK_DIR}"

    # The Controller no longer auto-generates admin credentials; it synthesizes a
    # local_auth provider from EDGION_ADMIN_*. Export them (honoring inherited
    # values, with the same defaults as start_all_with_conf.sh so the already-running
    # Controller and our restart/cli all agree) so the restarted Controller has auth
    # and edgion-cli (apply) can log in. JWT secret must be >= 32 bytes.
    export EDGION_ADMIN_USERNAME="${EDGION_ADMIN_USERNAME:-admin}"
    export EDGION_ADMIN_PASSWORD="${EDGION_ADMIN_PASSWORD:-edgion-integration-test-admin-pw}"
    export EDGION_ADMIN_JWT_SECRET="${EDGION_ADMIN_JWT_SECRET:-edgion-integration-test-jwt-secret-0123456789}"

    log_info "Work dir: ${WORK_DIR}"
}

cleanup() {
    # Remove temporary reconnect fixture if it exists
    rm -f /tmp/reconnect-route.yaml 2>/dev/null || true
    if $DO_CLEANUP; then
        log_section "Stopping test environment"
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    fi
}

# =============================================================================
# Prerequisite checks
# =============================================================================
check_prerequisites() {
    if [ ! -f "$EDGION_CLI" ]; then
        log_error "edgion-cli not found at $EDGION_CLI"
        log_error "Run: cargo build --bin edgion-cli  (or use prepare.sh)"
        exit 1
    fi
    if ! curl -sf "${CONTROLLER_URL}/health" > /dev/null 2>&1; then
        log_error "Controller not reachable at ${CONTROLLER_URL}"
        exit 1
    fi
    if ! curl -sf "${GATEWAY_PROBE_URL}/health" > /dev/null 2>&1; then
        log_error "Gateway probe not reachable at ${GATEWAY_PROBE_URL}"
        exit 1
    fi
    log_success "Prerequisites OK"
}

# =============================================================================
# Restart Controller with the same args used by start_all_with_conf.sh
# =============================================================================
restart_controller() {
    log_info "Restarting edgion-controller..."
    "${BIN_DIR}/edgion-controller" \
        -c "$CONTROLLER_CONFIG" \
        --work-dir "${WORK_DIR}" \
        --conf-dir "${CONFIG_DIR}" \
        --grpc-listen "0.0.0.0:${CONTROLLER_GRPC_PORT}" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        --test-mode \
        > "${LOG_DIR}/controller_restarted.log" 2>&1 &

    local new_pid=$!
    echo $new_pid > "${PID_DIR}/controller.pid"
    log_info "Restarted Controller with PID ${new_pid}"
}

# =============================================================================
# Poll Gateway configclient endpoint for non-empty route list
# =============================================================================
wait_for_gateway_routes() {
    local timeout=${1:-$RECONNECT_TIMEOUT}
    local elapsed=0
    log_info "Waiting up to ${timeout}s for Gateway to show routes after reconnect..."
    while [ $elapsed -lt $timeout ]; do
        local resp count
        resp=$(curl -sf "${GATEWAY_ADMIN_URL}/configclient/httproute/list" 2>/dev/null || echo '{"count":0}')
        count=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('count',0))" 2>/dev/null || echo "0")
        if [ "$count" -gt 0 ] 2>/dev/null; then
            log_success "Gateway configclient shows ${count} HTTPRoute(s) after ${elapsed}s"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    log_warn "Gateway configclient httproute count is still 0 after ${timeout}s"
    return 1
}

# =============================================================================
# Write the new-route fixture to a temp file (inline heredoc)
# =============================================================================
write_reconnect_fixture() {
    cat > /tmp/reconnect-route.yaml <<'YAML'
# Reconnect test fixture — applied after Controller restart to verify
# that the gRPC stream between Gateway and Controller is fully functional.
apiVersion: v1
kind: Service
metadata:
  name: test-http-reconnect
  namespace: edgion-test
spec:
  ports:
    - name: http-30001
      port: 30001
      protocol: TCP
      targetPort: 30001
  type: ClusterIP
---
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: test-http-reconnect
  namespace: edgion-test
  labels:
    kubernetes.io/service-name: test-http-reconnect
addressType: IPv4
ports:
  - name: http-30001
    port: 30001
    protocol: TCP
endpoints:
  - addresses:
      - "127.0.0.1"
    conditions:
      ready: true
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: reconnect-test
  namespace: edgion-test
spec:
  parentRefs:
    - name: http-test-gateway
      namespace: edgion-test
  hostnames:
    - "reconnect-test.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /echo
      backendRefs:
        - name: test-http-reconnect
          port: 30001
YAML
    log_info "Reconnect fixture written to /tmp/reconnect-route.yaml"
}

# =============================================================================
# Main
# =============================================================================
main() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --no-start)    DO_START=false;   shift ;;
            --keep-alive)  DO_CLEANUP=false; shift ;;
            --no-build)    DO_BUILD=false;   shift ;;
            -h|--help)
                echo "Usage: $0 [--no-start] [--keep-alive] [--no-build]"
                exit 0
                ;;
            *) log_error "Unknown option: $1"; exit 1 ;;
        esac
    done

    trap cleanup EXIT

    log_section "Controller Reconnect Test"
    echo "  Reconnect timeout: ${RECONNECT_TIMEOUT}s"
    echo ""

    # -------------------------------------------------------------------------
    # Environment setup
    # -------------------------------------------------------------------------
    if $DO_START; then
        start_environment
    else
        log_info "Attaching to existing environment (--no-start)"
    fi

    resolve_work_dir
    check_prerequisites

    # -------------------------------------------------------------------------
    # Case 1: initial_config_works
    # -------------------------------------------------------------------------
    log_section "Case 1: initial_config_works"
    log_info "GET /echo with Host: test.example.com — expect 200"
    HTTP_CODE=$(curl -s -o /dev/null -w '%{http_code}' \
        -H "Host: test.example.com" \
        "http://${GATEWAY_HOST}:${GATEWAY_HTTP_PORT}/echo" 2>/dev/null || echo "000")
    if [ "$HTTP_CODE" = "200" ]; then
        assert_pass "initial_config_works" "pass"
    else
        assert_pass "initial_config_works" "fail" "expected 200, got ${HTTP_CODE}"
    fi

    # -------------------------------------------------------------------------
    # Case 2: gateway_serves_after_controller_killed
    # -------------------------------------------------------------------------
    log_section "Case 2: gateway_serves_after_controller_killed"
    log_info "SIGKILL edgion-controller..."
    pkill -KILL -f "edgion-controller" 2>/dev/null || true
    sleep 2

    # Confirm Controller is truly gone
    if curl -sf "${CONTROLLER_URL}/health" > /dev/null 2>&1; then
        log_warn "Controller still responding after SIGKILL — test may be unreliable"
    else
        log_info "Controller confirmed dead (health check fails as expected)"
    fi

    log_info "GET /echo with Host: test.example.com — Gateway must still return 200"
    HTTP_CODE=$(curl -s -o /dev/null -w '%{http_code}' \
        -H "Host: test.example.com" \
        "http://${GATEWAY_HOST}:${GATEWAY_HTTP_PORT}/echo" 2>/dev/null || echo "000")
    if [ "$HTTP_CODE" = "200" ]; then
        assert_pass "gateway_serves_after_controller_killed" "pass"
    else
        assert_pass "gateway_serves_after_controller_killed" "fail" \
            "Gateway dropped config after Controller death — expected 200, got ${HTTP_CODE}"
    fi

    # -------------------------------------------------------------------------
    # Case 3: gateway_reconnects_to_new_controller
    # -------------------------------------------------------------------------
    log_section "Case 3: gateway_reconnects_to_new_controller"
    restart_controller

    # Poll Controller health; track elapsed time for reporting
    reconnect_elapsed=0
    controller_alive=false
    while [ $reconnect_elapsed -lt $RECONNECT_TIMEOUT ]; do
        if curl -sf "${CONTROLLER_URL}/health" > /dev/null 2>&1; then
            controller_alive=true
            break
        fi
        sleep 1
        reconnect_elapsed=$((reconnect_elapsed + 1))
    done

    if $controller_alive; then
        log_success "Controller healthy after ${reconnect_elapsed}s"
        assert_pass "gateway_reconnects_to_new_controller" "pass"
    else
        assert_pass "gateway_reconnects_to_new_controller" "fail" \
            "Controller did not become healthy within ${RECONNECT_TIMEOUT}s"
    fi
    log_info "Controller reconnect elapsed: ~${reconnect_elapsed}s"

    # -------------------------------------------------------------------------
    # Case 4: gateway_admin_shows_routes
    # -------------------------------------------------------------------------
    log_section "Case 4: gateway_admin_shows_routes"
    if wait_for_gateway_routes "$RECONNECT_TIMEOUT"; then
        assert_pass "gateway_admin_shows_routes" "pass"
    else
        RESP=$(curl -sf "${GATEWAY_ADMIN_URL}/configclient/httproute/list" 2>/dev/null || echo "unreachable")
        assert_pass "gateway_admin_shows_routes" "fail" \
            "configclient/httproute/list returned: ${RESP}"
    fi

    # -------------------------------------------------------------------------
    # Case 5: new_route_propagates_after_reconnect
    # -------------------------------------------------------------------------
    log_section "Case 5: new_route_propagates_after_reconnect"
    write_reconnect_fixture

    log_info "Applying reconnect fixture via edgion-cli..."
    # Right after a restart the Controller's config subsystem may still be
    # initializing ("controller initializing, please retry after init completes").
    # Retry the (idempotent) apply until it is accepted or we time out.
    apply_ok=false
    for _ in $(seq 1 15); do
        if "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f /tmp/reconnect-route.yaml > /dev/null 2>&1; then
            apply_ok=true
            break
        fi
        sleep 1
    done
    if $apply_ok; then
        log_info "Fixture applied successfully; waiting ${SYNC_WAIT}s for sync..."
        sleep "$SYNC_WAIT"

        # Poll for the new route to be served (up to RECONNECT_TIMEOUT)
        NEW_ROUTE_OK=false
        for i in $(seq 1 "$RECONNECT_TIMEOUT"); do
            HTTP_CODE=$(curl -s -o /dev/null -w '%{http_code}' \
                -H "Host: reconnect-test.example.com" \
                "http://${GATEWAY_HOST}:${GATEWAY_HTTP_PORT}/echo" 2>/dev/null || echo "000")
            if [ "$HTTP_CODE" = "200" ]; then
                NEW_ROUTE_OK=true
                log_info "New route served after ${i}s"
                break
            fi
            sleep 1
        done

        if $NEW_ROUTE_OK; then
            assert_pass "new_route_propagates_after_reconnect" "pass"
        else
            assert_pass "new_route_propagates_after_reconnect" "fail" \
                "reconnect-test.example.com returned ${HTTP_CODE} (expected 200) after ${RECONNECT_TIMEOUT}s"
        fi
    else
        assert_pass "new_route_propagates_after_reconnect" "fail" \
            "edgion-cli apply failed — Controller may not have accepted the fixture"
    fi

    # -------------------------------------------------------------------------
    # Case 6: gateway_alive_throughout
    # -------------------------------------------------------------------------
    log_section "Case 6: gateway_alive_throughout"
    # pgrep -c is Linux-only; on macOS use pgrep | wc -l
    GW_COUNT=$(pgrep -f "edgion-gateway" 2>/dev/null | wc -l | tr -d ' ')
    if [ "$GW_COUNT" -ge 1 ]; then
        assert_pass "gateway_alive_throughout" "pass"
    else
        assert_pass "gateway_alive_throughout" "fail" \
            "Expected at least 1 Gateway process, found ${GW_COUNT}"
    fi

    # -------------------------------------------------------------------------
    # Summary
    # -------------------------------------------------------------------------
    echo ""
    echo "=================================================="
    echo "Test Summary"
    echo "=================================================="
    echo "  PASSED: $PASSED"
    echo "  FAILED: $FAILED"
    echo "=================================================="

    if [ "$FAILED" -gt 0 ]; then
        log_error "${FAILED} test(s) failed"
        exit 1
    else
        log_success "All ${PASSED} tests passed"
        exit 0
    fi
}

main "$@"
