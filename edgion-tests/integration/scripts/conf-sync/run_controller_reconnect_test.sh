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
#      already-loaded config when the upstream Controller dies — and the Gateway
#      must RAISE confsync_disconnected (it kept serving, but it noticed).
#   5. Restart Controller (same args / config dir / port).
#   6. Wait for Gateway to reconnect: assert the Gateway's own
#      confsync_disconnected gauge returns to 0 (LINK_UP_TIMEOUT, default 90s —
#      bounded by the Gateway's reconnect backoff, not the Controller's boot).
#      Controller /health is a precondition check only — it says nothing about
#      whether THIS Gateway reattached.
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
CONTROLLER_URL="${EDGION_CONTROLLER_URL:-http://127.0.0.1:12101}"
CONTROLLER_PROBE_URL="${CONTROLLER_PROBE_URL:-http://127.0.0.1:12100}"
GATEWAY_ADMIN_URL="${EDGION_GATEWAY_ADMIN_URL:-http://127.0.0.1:12001}"
# Gateway admin API requires a bearer token; default for standalone invocation.
: "${EDGION_GATEWAY_ADMIN_TOKEN:=integration-gateway-admin-token-0123456789}"
GATEWAY_PROBE_URL="${EDGION_GATEWAY_PROBE_URL:-http://127.0.0.1:12000}"
# Gateway Prometheus listener (MetricsConfig::default().listen) — carries the
# conf_sync link-health gauge the reconnect assertions below read.
GATEWAY_METRICS_URL="${EDGION_GATEWAY_METRICS_URL:-http://127.0.0.1:12090/metrics}"
CONTROLLER_ADMIN_PORT=12101
CONTROLLER_GRPC_PORT="${EDGION_TEST_CONTROLLER_GRPC_PORT:-12151}"

RECONNECT_TIMEOUT=30   # seconds to wait for Gateway to reconnect after Controller restart
SYNC_WAIT=3            # seconds to wait after applying new route before verification
# Budget for the Gateway's conf_sync link to come back UP. Deliberately much larger
# than RECONNECT_TIMEOUT: this one is not bounded by how fast the Controller boots,
# but by the Gateway's own reconnect backoff (start_watch_server_meta: 1s doubling
# to a 30s cap). The Controller takes ~16s to init, during which the Gateway keeps
# failing and its backoff climbs to the cap — so the Gateway can be mid-30s-sleep
# exactly when the Controller becomes ready, putting a legitimate reconnect at
# ~46s. A 30s budget here passed with 2s to spare on a quiet dev box and would
# flake on a loaded CI runner.
LINK_UP_TIMEOUT=90

PASSED=0
FAILED=0

DO_START=true
DO_CLEANUP=true
DO_BUILD=true

WORK_DIR=""
LOG_DIR=""
PID_DIR=""
CONFIG_DIR=""
# Resolved in resolve_work_dir() to the WORK-DIR COPY that start_all_with_conf.sh produced.
# The repo config/edgion-controller.yaml carries the same (mTLS) transport block, but its
# work-dir relative cert paths only resolve against the run's WORK_DIR — which is where
# start_all_with_conf.sh distributed the cert set. Restart from the work-dir copy with
# --work-dir "${WORK_DIR}" so the Controller finds the certs the running Gateway trusts.
CONTROLLER_CONFIG=""

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
    local current_file="${PROJECT_ROOT}/tmp/integration_testing/.current"
    if [ ! -f "$current_file" ]; then
        log_error "tmp/integration_testing/.current not found — run start_all_with_conf.sh first"
        exit 1
    fi
    WORK_DIR=$(cat "$current_file")
    LOG_DIR="${WORK_DIR}/logs"
    PID_DIR="${WORK_DIR}/pids"
    CONFIG_DIR="${WORK_DIR}/config"
    # The work-dir copy of the checked-in (mTLS) config; its cert paths resolve under WORK_DIR.
    CONTROLLER_CONFIG="${CONFIG_DIR}/edgion-controller.yaml"
    if [ ! -f "$CONTROLLER_CONFIG" ]; then
        log_error "Controller config not found: ${CONTROLLER_CONFIG} -- run start_all_with_conf.sh first"
        exit 1
    fi

    export EDGION_WORK_DIR="${WORK_DIR}"

    # Token-only admin auth. The Controller reads a single pre-shared bearer token
    # from EDGION_ADMIN_TOKEN. Export it (honoring an inherited value, with the same
    # default literal as start_all_with_conf.sh so the already-running Controller and
    # our restarted Controller / edgion-cli all agree). edgion-cli (apply) reads the
    # same value via EDGION_TOKEN.
    export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
    export EDGION_TOKEN="${EDGION_ADMIN_TOKEN}"

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
    if ! curl -sf "${CONTROLLER_PROBE_URL}/health" > /dev/null 2>&1; then
        log_error "Controller not reachable at ${CONTROLLER_PROBE_URL}"
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
        resp=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/configclient/httproute/list" 2>/dev/null || echo '{"count":0}')
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
# Gateway-side conf_sync link health
#
# Reads edgion_system_error_state{reason="confsync_disconnected"} off the
# Gateway's own metrics listener: 1 = its WatchServerMeta stream to the
# Controller is down and it is serving last-known config; 0 = the stream is
# established. This is the only assertion here that the GATEWAY makes about the
# link — polling the Controller's /health proves the Controller is up, not that
# this Gateway reattached to it (a Gateway left on plaintext against a TLS
# Controller passes every such check while never syncing again).
#
# The series exists from Gateway startup (the first successful connect clears
# it to 0), so an empty result means the metrics listener is unreachable, not
# that the link is healthy — the callers below treat "" as not-yet-matching.
# =============================================================================
gw_confsync_disconnected() {
    # Pin BOTH labels: matching on `reason` alone would match two lines the day any
    # other component raises the same reason, and `head -1` would then silently pick
    # whichever the exporter rendered first (registry order is not a contract).
    # Chained greps rather than one regex on purpose — a single pattern would have to
    # fix an order for the two labels, and their render order is not a contract either.
    # `^` + literal `{` excludes the `# HELP` / `# TYPE` lines and any sibling metric
    # whose name merely starts the same. `$NF+0` normalizes the exporter's float
    # rendering (1 / 1.0) to an integer.
    curl -sf "${GATEWAY_METRICS_URL}" 2>/dev/null \
        | grep -E '^edgion_system_error_state\{' \
        | grep -F 'component="conf_sync_client"' \
        | grep -F 'reason="confsync_disconnected"' \
        | awk '{print $NF+0}' \
        | head -1
}

# Poll until the Gateway's conf_sync gauge reaches $1 (0 = linked, 1 = down).
wait_for_confsync_state() {
    local want=$1
    local timeout=${2:-$RECONNECT_TIMEOUT}
    local label=$3
    local elapsed=0
    log_info "Waiting up to ${timeout}s for Gateway conf_sync link to be ${label}..."
    while [ $elapsed -lt $timeout ]; do
        if [ "$(gw_confsync_disconnected)" = "$want" ]; then
            log_success "Gateway conf_sync link ${label} after ${elapsed}s"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    log_warn "Gateway conf_sync gauge is '$(gw_confsync_disconnected)' (wanted ${want}) after ${timeout}s"
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
    # Case 1b: gateway_reports_confsync_up_initially
    #
    # Pins the gauge's healthy baseline. Without this, a gauge stuck at 1 from
    # startup would make Case 2b ("link went down") pass vacuously — the exact
    # failure mode this suite exists to catch.
    # -------------------------------------------------------------------------
    log_section "Case 1b: gateway_reports_confsync_up_initially"
    # `|| true` is load-bearing: under `set -euo pipefail` a bare assignment takes the
    # command substitution's status, so an unreachable metrics listener (curl -sf) or an
    # absent series (grep) would abort the whole script here — killing every later case
    # and printing no diagnosis. The empty-string branch below must stay reachable.
    INITIAL_LINK=$(gw_confsync_disconnected || true)
    if [ "$INITIAL_LINK" = "0" ]; then
        assert_pass "gateway_reports_confsync_up_initially" "pass"
    else
        assert_pass "gateway_reports_confsync_up_initially" "fail" \
            "expected confsync_disconnected=0 on a freshly synced Gateway, got '${INITIAL_LINK}' (empty = metrics listener unreachable at ${GATEWAY_METRICS_URL})"
    fi

    # -------------------------------------------------------------------------
    # Case 2: gateway_serves_after_controller_killed
    # -------------------------------------------------------------------------
    log_section "Case 2: gateway_serves_after_controller_killed"
    log_info "SIGKILL edgion-controller..."
    pkill -KILL -f "edgion-controller" 2>/dev/null || true
    sleep 2

    # Confirm Controller is truly gone
    if curl -sf "${CONTROLLER_PROBE_URL}/health" > /dev/null 2>&1; then
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
    # Case 2b: gateway_reports_confsync_down
    #
    # Serving 200 above proves the Gateway kept its config; it does NOT prove the
    # Gateway noticed the Controller is gone. Without this, a Gateway that silently
    # drifts out of sync forever looks identical to a healthy one.
    # -------------------------------------------------------------------------
    log_section "Case 2b: gateway_reports_confsync_down"
    LINK_WENT_DOWN=false
    if wait_for_confsync_state 1 "$RECONNECT_TIMEOUT" "DOWN"; then
        LINK_WENT_DOWN=true
        assert_pass "gateway_reports_confsync_down" "pass"
    else
        assert_pass "gateway_reports_confsync_down" "fail" \
            "Gateway did not raise confsync_disconnected after the Controller was killed"
    fi

    # -------------------------------------------------------------------------
    # Case 3: gateway_reconnects_to_new_controller
    # -------------------------------------------------------------------------
    log_section "Case 3: gateway_reconnects_to_new_controller"
    restart_controller

    # Poll Controller health first — this only establishes that the restarted
    # Controller is up, which is a precondition for the real assertion below, not
    # evidence of a reconnect. Kept non-fatal for that reason: the Gateway-side
    # gauge is what decides this case.
    reconnect_elapsed=0
    controller_alive=false
    while [ $reconnect_elapsed -lt $RECONNECT_TIMEOUT ]; do
        if curl -sf "${CONTROLLER_PROBE_URL}/health" > /dev/null 2>&1; then
            controller_alive=true
            break
        fi
        sleep 1
        reconnect_elapsed=$((reconnect_elapsed + 1))
    done
    if $controller_alive; then
        log_info "Controller back up after ${reconnect_elapsed}s; now checking the Gateway reattached"
    else
        log_warn "Controller did not report healthy within ${RECONNECT_TIMEOUT}s"
    fi

    # The actual assertion: a 0->1->0 TRANSITION, not just a 0 now.
    #
    # Requiring case 2b's observed 1 is what stops this from passing vacuously: a gauge
    # that never rises at all (a regressed mark, or a code path that skips it) sits at 0
    # from startup, and a bare level check would then report "link UP after 0s" — PASS,
    # having proven nothing about a reconnect. That is the same disease this suite was
    # rewritten to cure, so do not "simplify" the LINK_WENT_DOWN gate away.
    #
    # Budgeted by LINK_UP_TIMEOUT, not RECONNECT_TIMEOUT — see that variable for why the
    # Gateway's own backoff, not the Controller's boot, sets the worst case.
    if ! $LINK_WENT_DOWN; then
        assert_pass "gateway_reconnects_to_new_controller" "fail" \
            "cannot judge a reconnect: the link was never observed DOWN (case 2b failed), so a 0 here would prove nothing"
    elif wait_for_confsync_state 0 "$LINK_UP_TIMEOUT" "UP"; then
        assert_pass "gateway_reconnects_to_new_controller" "pass"
    else
        assert_pass "gateway_reconnects_to_new_controller" "fail" \
            "Gateway never cleared confsync_disconnected — it has not resynced with the restarted Controller (controller_alive=${controller_alive})"
    fi

    # -------------------------------------------------------------------------
    # Case 4: gateway_admin_shows_routes
    # -------------------------------------------------------------------------
    log_section "Case 4: gateway_admin_shows_routes"
    if wait_for_gateway_routes "$RECONNECT_TIMEOUT"; then
        assert_pass "gateway_admin_shows_routes" "pass"
    else
        RESP=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/configclient/httproute/list" 2>/dev/null || echo "unreachable")
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
    #
    # The budget must comfortably exceed real init time: it was measured at ~16s against a
    # 15x1s budget, so this case failed by ~1s and looked like a propagation bug. Keep the
    # headroom generous — a budget that sits just under init time is a flake generator, not
    # a bug detector.
    #
    # Keep the last error, too: swallowing it into /dev/null made a genuine apply rejection
    # indistinguishable from "still initializing", which is exactly how the ~1s shortfall got
    # misread as the Controller never leaving init at all.
    apply_ok=false
    apply_err=""
    for _ in $(seq 1 60); do
        if apply_err=$("$EDGION_CLI" --server "$CONTROLLER_URL" apply -f /tmp/reconnect-route.yaml 2>&1); then
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
            "edgion-cli apply never accepted within 60s; last error: ${apply_err}"
    fi

    # -------------------------------------------------------------------------
    # Case 6: gateway_alive_throughout
    # -------------------------------------------------------------------------
    log_section "Case 6: gateway_alive_throughout"
    # pgrep -c is Linux-only; on macOS use pgrep | wc -l.
    # `|| true` is load-bearing for the same reason as case 1b's: pgrep exits 1 when it
    # matches nothing, pipefail propagates that, and a bare assignment inherits it — so
    # under `set -e` a DEAD Gateway would abort the script here instead of failing this
    # case, discarding the FAIL and the whole summary. The one condition this case exists
    # to catch is exactly the one that would silence it.
    GW_COUNT=$(pgrep -f "edgion-gateway" 2>/dev/null | wc -l | tr -d ' ' || true)
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
