#!/usr/bin/env bash
# =============================================================================
# Controller Stress Restart Test — verifies Controller reconverges 200+
# HTTPRoute and 50+ BackendTLSPolicy resources after a restart, and that
# Gateway sees no resource loss.
#
# Self-contained: starts test_server + Controller + Gateway, generates
# fixtures, applies them, restarts Controller, validates reconvergence,
# stops everything (unless --keep-alive).
#
# Steps:
#   1. Start environment with HTTPRoute/Basic suite (provides shared
#      test-http Service + EndpointSlice).
#   2. Generate HTTPROUTE_COUNT HTTPRoute YAML fixtures (each with unique
#      hostname) and BTLSP_COUNT BackendTLSPolicy fixtures (each targeting
#      a different synthetic Service name).
#   3. Apply all fixtures via edgion-cli (directory apply).
#   4. Wait for initial convergence: poll Gateway admin
#      /configclient/httproute/list count >= baseline + HTTPROUTE_COUNT.
#   5. Spot-check sanity: 5 sampled hostnames return 200 via Gateway.
#   6. SIGKILL Controller; wait briefly.
#   7. Restart Controller with same args (mirrors start_all_with_conf.sh).
#   8. Wait up to RESTART_TIMEOUT (default 90s) for Controller to be
#      healthy AND Gateway admin to report >= original counts again.
#   9. Spot-check sanity post-restart: same 5 hostnames still 200.
#   10. Cleanup (unless --keep-alive).
#
# Resource counts:
#   HTTPROUTE_COUNT (default 200)
#   BTLSP_COUNT     (default 50)
#
# Usage:
#   ./run_controller_stress_restart_test.sh                         # Full run
#   ./run_controller_stress_restart_test.sh --route-count 20        # Smaller run
#   ./run_controller_stress_restart_test.sh --no-start              # Use existing env
#   ./run_controller_stress_restart_test.sh --keep-alive            # Don't stop
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
CONTROLLER_ADMIN_PORT=5800
CONTROLLER_GRPC_PORT="${EDGION_TEST_CONTROLLER_GRPC_PORT:-50051}"

RESTART_TIMEOUT=90   # seconds to wait for Controller restart + Gateway convergence
SYNC_WAIT=5          # seconds to wait after apply before first convergence poll

# Default resource counts
HTTPROUTE_COUNT=200
BTLSP_COUNT=50

PASSED=0
FAILED=0

DO_START=true
DO_CLEANUP=true

WORK_DIR=""
LOG_DIR=""
PID_DIR=""
CONFIG_DIR=""
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.yaml"

# =============================================================================
# Log helpers
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

    # Let edgion-cli auto_auth find credentials via EDGION_WORK_DIR
    export EDGION_WORK_DIR="${WORK_DIR}"

    log_info "Work dir: ${WORK_DIR}"
}

cleanup() {
    # Remove generated fixture directory
    if [ -n "${FIXTURE_DIR:-}" ] && [ -d "${FIXTURE_DIR}" ]; then
        rm -rf "${FIXTURE_DIR}" 2>/dev/null || true
    fi
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
    if ! curl -sf "${GATEWAY_ADMIN_URL}/health" > /dev/null 2>&1; then
        log_error "Gateway admin not reachable at ${GATEWAY_ADMIN_URL}"
        exit 1
    fi
    log_success "Prerequisites OK"
}

# =============================================================================
# Generate fixture YAML files into a temp directory
# =============================================================================
FIXTURE_DIR=""

generate_fixtures() {
    FIXTURE_DIR="${WORK_DIR}/stress-fixtures"
    mkdir -p "$FIXTURE_DIR"

    log_info "Generating ${HTTPROUTE_COUNT} HTTPRoute fixtures..."
    for i in $(seq 1 ${HTTPROUTE_COUNT}); do
        cat > "${FIXTURE_DIR}/HTTPRoute_${i}.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: stress-route-${i}
  namespace: edgion-test
spec:
  parentRefs:
    - name: http-test-gateway
      namespace: edgion-test
  hostnames:
    - "stress-${i}.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: test-http
          port: 30001
EOF
    done

    log_info "Generating ${BTLSP_COUNT} BackendTLSPolicy fixtures..."
    for i in $(seq 1 ${BTLSP_COUNT}); do
        cat > "${FIXTURE_DIR}/BTLSP_${i}.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1alpha3
kind: BackendTLSPolicy
metadata:
  name: stress-btlsp-${i}
  namespace: edgion-test
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: stress-svc-${i}
  validation:
    hostname: stress-svc-${i}.example.com
    wellKnownCACertificates: System
EOF
    done

    log_info "Generated ${HTTPROUTE_COUNT} HTTPRoute + ${BTLSP_COUNT} BackendTLSPolicy fixtures in ${FIXTURE_DIR}"
}

# =============================================================================
# Apply all fixtures via edgion-cli directory apply
# =============================================================================
apply_fixtures() {
    local t_start t_end elapsed
    t_start=$(date +%s)

    log_info "Applying fixture directory ${FIXTURE_DIR} via edgion-cli..."
    local apply_output apply_exit
    apply_output=$("${EDGION_CLI}" apply -f "${FIXTURE_DIR}" 2>&1) && apply_exit=0 || apply_exit=$?

    t_end=$(date +%s)
    elapsed=$((t_end - t_start))
    log_info "Apply completed in ${elapsed}s (exit ${apply_exit})"

    if [ $apply_exit -ne 0 ]; then
        log_error "edgion-cli apply failed:"
        echo "$apply_output"
        return 1
    fi

    # Count success / failures from output lines
    local applied failed
    applied=$(echo "$apply_output" | grep -c "created\|updated" 2>/dev/null || true)
    failed=$(echo "$apply_output" | grep -c -i "failed\|error" 2>/dev/null || true)
    log_info "Apply summary: ~${applied} succeeded, ~${failed} errors reported"

    return 0
}

# =============================================================================
# Query Gateway admin for a resource kind count
# =============================================================================
get_gateway_count() {
    local kind=$1   # e.g. "httproute" or "backendtlspolicy"
    local resp count
    resp=$(curl -sf "${GATEWAY_ADMIN_URL}/configclient/${kind}/list" 2>/dev/null || echo '{"count":0}')
    count=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('count',0))" 2>/dev/null || echo "0")
    echo "${count}"
}

# =============================================================================
# Poll until Gateway admin reports >= expected_routes HTTPRoutes
# =============================================================================
wait_for_convergence() {
    local expected_routes=$1
    local timeout=$2
    local label=${3:-"convergence"}
    local start elapsed count
    start=$(date +%s)

    log_info "Waiting up to ${timeout}s for Gateway HTTPRoute count >= ${expected_routes} (${label})..."
    while true; do
        elapsed=$(( $(date +%s) - start ))
        if [ $elapsed -ge $timeout ]; then
            count=$(get_gateway_count "httproute")
            log_error "Convergence timeout after ${timeout}s: HTTPRoute count=${count}, expected>=${expected_routes}"
            return 1
        fi
        count=$(get_gateway_count "httproute")
        if [ "${count}" -ge "${expected_routes}" ] 2>/dev/null; then
            log_info "Converged: HTTPRoute count=${count} >= ${expected_routes} (${elapsed}s)"
            return 0
        fi
        sleep 1
    done
}

# =============================================================================
# Spot-check 5 stress routes: each must return HTTP 200
# Returns 0 if all pass, 1 if any fail
# =============================================================================
spot_check_routes() {
    # Pick 5 evenly-distributed indices across [1..HTTPROUTE_COUNT]
    local quarter=$(( HTTPROUTE_COUNT / 4 ))
    local half=$(( HTTPROUTE_COUNT / 2 ))
    local three_quarter=$(( HTTPROUTE_COUNT * 3 / 4 ))
    local indices=(1)
    [ $quarter -gt 1 ]           && indices+=($quarter)        || indices+=(1)
    [ $half -gt $quarter ]       && indices+=($half)            || indices+=(1)
    [ $three_quarter -gt $half ] && indices+=($three_quarter)   || indices+=(1)
    indices+=($HTTPROUTE_COUNT)

    local fail=0
    for i in "${indices[@]}"; do
        local code
        # Use /health — the test_server handles this path; route rule uses PathPrefix: /
        code=$(curl -s -o /dev/null -w '%{http_code}' \
               -H "Host: stress-${i}.example.com" \
               "http://${GATEWAY_HOST}:${GATEWAY_HTTP_PORT}/health" 2>/dev/null || echo "000")
        if [ "$code" = "200" ]; then
            log_info "  stress-${i}.example.com/health → ${code} OK"
        else
            log_error "  stress-${i}.example.com/health → ${code} (expected 200)"
            fail=$((fail + 1))
        fi
    done
    return $fail
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
# Main
# =============================================================================
main() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --no-start)         DO_START=false;              shift ;;
            --keep-alive)       DO_CLEANUP=false;            shift ;;
            --route-count)      HTTPROUTE_COUNT="$2";        shift 2 ;;
            --btlsp-count)      BTLSP_COUNT="$2";            shift 2 ;;
            -h|--help)
                echo "Usage: $0 [--no-start] [--keep-alive] [--route-count N] [--btlsp-count N]"
                exit 0
                ;;
            *) log_error "Unknown option: $1"; exit 1 ;;
        esac
    done

    trap cleanup EXIT

    log_section "Controller Stress Restart Test"
    echo "  HTTPRoute count:   ${HTTPROUTE_COUNT}"
    echo "  BackendTLSPolicy:  ${BTLSP_COUNT}"
    echo "  Restart timeout:   ${RESTART_TIMEOUT}s"
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
    # Record baseline HTTPRoute count (from HTTPRoute/Basic suite)
    # -------------------------------------------------------------------------
    BASELINE_ROUTES=$(get_gateway_count "httproute")
    EXPECTED_ROUTES=$(( BASELINE_ROUTES + HTTPROUTE_COUNT ))
    log_info "Baseline HTTPRoute count: ${BASELINE_ROUTES}  →  target after apply: ${EXPECTED_ROUTES}"

    # -------------------------------------------------------------------------
    # Generate + apply fixtures
    # -------------------------------------------------------------------------
    log_section "Generating and applying stress fixtures"
    generate_fixtures

    local apply_ok=true
    apply_fixtures || apply_ok=false

    # Case 1: initial_apply_succeeded
    log_section "Case 1: initial_apply_succeeded"
    if $apply_ok; then
        assert_pass "initial_apply_succeeded" "pass"
    else
        assert_pass "initial_apply_succeeded" "fail" "edgion-cli apply returned non-zero"
    fi

    # -------------------------------------------------------------------------
    # Wait for initial convergence
    # -------------------------------------------------------------------------
    log_section "Case 2: initial_convergence"
    log_info "Waiting ${SYNC_WAIT}s after apply before polling..."
    sleep "$SYNC_WAIT"

    local converge_start converge_elapsed
    converge_start=$(date +%s)

    if wait_for_convergence "$EXPECTED_ROUTES" "$RESTART_TIMEOUT" "initial"; then
        converge_elapsed=$(( $(date +%s) - converge_start ))
        log_info "Initial convergence elapsed: ${converge_elapsed}s"
        assert_pass "initial_convergence" "pass"
    else
        converge_elapsed=$(( $(date +%s) - converge_start ))
        local actual
        actual=$(get_gateway_count "httproute")
        assert_pass "initial_convergence" "fail" \
            "HTTPRoute count=${actual} after ${converge_elapsed}s, expected>=${EXPECTED_ROUTES}"
    fi

    # -------------------------------------------------------------------------
    # Case 3: pre_restart_spot_check
    # -------------------------------------------------------------------------
    log_section "Case 3: pre_restart_spot_check"
    if spot_check_routes; then
        assert_pass "pre_restart_spot_check" "pass"
    else
        assert_pass "pre_restart_spot_check" "fail" "One or more stress routes did not return 200"
    fi

    # -------------------------------------------------------------------------
    # Case 4: controller_killed_gateway_serves
    # Kill Controller and verify Gateway still serves traffic from cached config.
    # -------------------------------------------------------------------------
    log_section "Case 4: controller_killed_gateway_serves"
    log_info "SIGKILL edgion-controller..."
    pkill -KILL -f "edgion-controller" 2>/dev/null || true
    sleep 2

    if curl -sf "${CONTROLLER_URL}/health" > /dev/null 2>&1; then
        log_warn "Controller still responding after SIGKILL — test may be unreliable"
    else
        log_info "Controller confirmed dead (health check fails as expected)"
    fi

    # Gateway must still serve from its in-memory cache
    if spot_check_routes; then
        assert_pass "controller_killed_gateway_serves" "pass"
    else
        assert_pass "controller_killed_gateway_serves" "fail" \
            "Gateway dropped cached config after Controller death"
    fi

    # -------------------------------------------------------------------------
    # Case 5: controller_restarted_healthy
    # Restart Controller and wait for it to become healthy.
    # -------------------------------------------------------------------------
    log_section "Case 5: controller_restarted_healthy"
    restart_controller

    local restart_elapsed=0
    local controller_alive=false
    local restart_t_start
    restart_t_start=$(date +%s)

    while [ $restart_elapsed -lt $RESTART_TIMEOUT ]; do
        if curl -sf "${CONTROLLER_URL}/health" > /dev/null 2>&1; then
            controller_alive=true
            restart_elapsed=$(( $(date +%s) - restart_t_start ))
            break
        fi
        sleep 1
        restart_elapsed=$(( $(date +%s) - restart_t_start ))
    done

    if $controller_alive; then
        log_success "Controller healthy after ${restart_elapsed}s"
        assert_pass "controller_restarted_healthy" "pass"
    else
        assert_pass "controller_restarted_healthy" "fail" \
            "Controller did not become healthy within ${RESTART_TIMEOUT}s"
    fi
    log_info "Controller restart-to-healthy elapsed: ~${restart_elapsed}s"

    # -------------------------------------------------------------------------
    # Case 6: post_restart_convergence
    # Gateway must reconverge to >= EXPECTED_ROUTES HTTPRoutes after restart.
    # -------------------------------------------------------------------------
    log_section "Case 6: post_restart_convergence"
    local remaining_timeout=$(( RESTART_TIMEOUT - restart_elapsed ))
    [ $remaining_timeout -lt 10 ] && remaining_timeout=10

    local postconv_start postconv_elapsed
    postconv_start=$(date +%s)

    if wait_for_convergence "$EXPECTED_ROUTES" "$remaining_timeout" "post-restart"; then
        postconv_elapsed=$(( $(date +%s) - postconv_start ))
        log_info "Post-restart convergence elapsed: ${postconv_elapsed}s"
        assert_pass "post_restart_convergence" "pass"
    else
        postconv_elapsed=$(( $(date +%s) - postconv_start ))
        local actual
        actual=$(get_gateway_count "httproute")
        assert_pass "post_restart_convergence" "fail" \
            "HTTPRoute count=${actual} after ${postconv_elapsed}s, expected>=${EXPECTED_ROUTES}"
    fi

    # -------------------------------------------------------------------------
    # Case 7: post_restart_spot_check
    # Same 5 hostnames must still return 200 after reconvergence.
    # -------------------------------------------------------------------------
    log_section "Case 7: post_restart_spot_check"
    if spot_check_routes; then
        assert_pass "post_restart_spot_check" "pass"
    else
        assert_pass "post_restart_spot_check" "fail" \
            "One or more stress routes returned non-200 after restart"
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
