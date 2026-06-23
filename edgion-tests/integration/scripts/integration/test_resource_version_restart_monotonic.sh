#!/usr/bin/env bash
# =============================================================================
# Resource Version Restart Monotonicity Test
#
# Verifies that the in-process resource_version counter is reconstructed from
# disk during init, so a controller restart never assigns an rv smaller than
# anything already persisted.
#
# Phase 1 (in-session monotonicity):
#   - POST a fresh HTTPRoute via the admin API; record metadata.resourceVersion
#     (rv1).
#   - PUT the same route (update); record rv2. Assert rv2 > rv1.
#
# Phase 2 (cross-restart monotonicity):
#   - SIGKILL the controller; wait until both the process is gone AND the
#     admin/gRPC ports are free.
#   - Restart the controller with the same args.
#   - Poll /ready until 200 (proves require_ready_middleware will let writes
#     through; /ready is gated on the same is_ready() check as write routes).
#   - PUT the route once more; record rv_after. Assert rv_after > rv2. This
#     proves bump_resource_version_floor reconstructed the floor from disk
#     during init.
#
# Modeled on edgion-tests/integration/scripts/conf-sync/run_controller_reconnect_test.sh
# for the restart pattern and on edgion-tests/integration/scripts/integration/
# test_file_watcher.sh for the admin auth pattern.
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
BIN_DIR="${PROJECT_ROOT}/target/debug"

CONTROLLER_HOST="${EDGION_CONTROLLER_HOST:-127.0.0.1}"
CONTROLLER_ADMIN_PORT="${EDGION_CONTROLLER_ADMIN_PORT:-12101}"
CONTROLLER_URL="http://${CONTROLLER_HOST}:${CONTROLLER_ADMIN_PORT}"
CONTROLLER_PROBE_PORT="${EDGION_CONTROLLER_PROBE_PORT:-12100}"
CONTROLLER_PROBE_URL="http://${CONTROLLER_HOST}:${CONTROLLER_PROBE_PORT}"
CONTROLLER_GRPC_PORT="${EDGION_TEST_CONTROLLER_GRPC_PORT:-12151}"
RESTART_TIMEOUT=30
PORT_FREE_TIMEOUT=15

FIXTURE_NS="edgion-test"
FIXTURE_NAME="rv-monotonic-probe"
FIXTURE_PATH="/tmp/rv-monotonic-probe.yaml"

PASSED=0
FAILED=0

DO_START=true
DO_CLEANUP=true
STARTED_BY_US=false

WORK_DIR=""
LOG_DIR=""
PID_DIR=""
CONFIG_DIR=""
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.yaml"
ADMIN_TOKEN=""

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_error()   { echo -e "${RED}[FAIL]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

assert_pass() {
    local name=$1
    local result=$2
    local detail=${3:-""}
    if [ "$result" = "pass" ]; then
        log_success "PASS: ${name}"
        PASSED=$((PASSED + 1))
    else
        log_error "FAIL: ${name}${detail:+ -- }${detail}"
        FAILED=$((FAILED + 1))
    fi
}

# =============================================================================
# Environment lifecycle
# =============================================================================
start_environment() {
    log_section "Starting test environment (HTTPRoute/Basic)"
    if ! "${UTILS_DIR}/start_all_with_conf.sh" --suites HTTPRoute/Basic; then
        log_error "Failed to start test environment"
        exit 1
    fi
    STARTED_BY_US=true
    log_success "Test environment started"
}

resolve_work_dir() {
    local current_file="${PROJECT_ROOT}/tmp/integration_testing/.current"
    if [ ! -f "$current_file" ]; then
        log_error "tmp/integration_testing/.current not found -- run start_all_with_conf.sh first"
        exit 1
    fi
    WORK_DIR=$(cat "$current_file")
    LOG_DIR="${WORK_DIR}/logs"
    PID_DIR="${WORK_DIR}/pids"
    CONFIG_DIR="${WORK_DIR}/config"
    export EDGION_WORK_DIR="${WORK_DIR}"
    log_info "Work dir: ${WORK_DIR}"
}

cleanup() {
    rm -f "${FIXTURE_PATH}" 2>/dev/null || true
    # Best-effort delete via admin API; ignore failure (controller may already be gone).
    if [ -n "$ADMIN_TOKEN" ]; then
        curl -s -o /dev/null -X DELETE -H "Authorization: Bearer ${ADMIN_TOKEN}" \
            "${CONTROLLER_URL}/api/v1/namespaced/HTTPRoute/${FIXTURE_NS}/${FIXTURE_NAME}" 2>/dev/null || true
    fi
    # Only tear down the environment when this script started it. A user
    # invoking `--no-start` keeps responsibility for their own daemons.
    if $DO_CLEANUP && $STARTED_BY_US; then
        log_section "Stopping test environment"
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    fi
}

check_prerequisites() {
    if [ ! -x "${BIN_DIR}/edgion-controller" ]; then
        log_error "edgion-controller not found at ${BIN_DIR}/edgion-controller"
        log_error "Run: cargo build --bin edgion-controller (or use prepare.sh)"
        exit 1
    fi
    if ! curl -sf "${CONTROLLER_PROBE_URL}/health" > /dev/null 2>&1; then
        log_error "Controller not reachable at ${CONTROLLER_PROBE_URL}"
        exit 1
    fi
    log_success "Prerequisites OK"
}

# =============================================================================
# Admin auth (token-only model): the Controller reads a single pre-shared bearer
# token from EDGION_ADMIN_TOKEN (honored by every Controller (re)start below). We
# export it and adopt that same value as the Bearer credential. Mirrors
# test_file_watcher.sh.
# =============================================================================
export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
ensure_admin_token() {
    if [ -n "$ADMIN_TOKEN" ]; then
        return 0
    fi
    if [ -z "${EDGION_ADMIN_TOKEN:-}" ]; then
        log_error "EDGION_ADMIN_TOKEN not set — cannot authenticate to controller admin API"
        exit 1
    fi
    ADMIN_TOKEN="${EDGION_ADMIN_TOKEN}"
}

# =============================================================================
# Controller restart with port-free polling
# =============================================================================
wait_port_free() {
    local port=$1
    local timeout=${2:-$PORT_FREE_TIMEOUT}
    # `lsof` is portable on macOS + most Linux distros. On minimal containers
    # it may be missing; fall back to a fixed sleep so the script still makes
    # forward progress (the caller logs a warning on overall failure).
    if ! command -v lsof > /dev/null 2>&1; then
        log_warn "lsof not found -- falling back to fixed 5s sleep for port ${port}"
        sleep 5
        return 0
    fi
    local elapsed=0
    while [ $elapsed -lt $timeout ]; do
        if ! lsof -i ":${port}" > /dev/null 2>&1; then
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    return 1
}

restart_controller() {
    log_info "Killing edgion-controller..."
    pkill -KILL -f "edgion-controller" 2>/dev/null || true

    log_info "Waiting for admin port ${CONTROLLER_ADMIN_PORT} to be released..."
    if ! wait_port_free "${CONTROLLER_ADMIN_PORT}"; then
        log_warn "Admin port still bound after ${PORT_FREE_TIMEOUT}s -- restart may EADDRINUSE"
    fi
    log_info "Waiting for gRPC port ${CONTROLLER_GRPC_PORT} to be released..."
    if ! wait_port_free "${CONTROLLER_GRPC_PORT}"; then
        log_warn "gRPC port still bound after ${PORT_FREE_TIMEOUT}s -- restart may EADDRINUSE"
    fi

    log_info "Restarting edgion-controller..."
    "${BIN_DIR}/edgion-controller" \
        -c "$CONTROLLER_CONFIG" \
        --work-dir "${WORK_DIR}" \
        --conf-dir "${CONFIG_DIR}" \
        --grpc-listen "0.0.0.0:${CONTROLLER_GRPC_PORT}" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        --test-mode \
        > "${LOG_DIR}/controller_rv_restart.log" 2>&1 &
    local new_pid=$!
    echo $new_pid > "${PID_DIR}/controller.pid"
    log_info "Restarted Controller with PID ${new_pid}"

    # Wait for /ready (not /health) -- /ready returns 200 only when is_ready()
    # is true, which is exactly the predicate that gates write routes via
    # require_ready_middleware.
    local elapsed=0
    while [ $elapsed -lt $RESTART_TIMEOUT ]; do
        if curl -sf "${CONTROLLER_PROBE_URL}/ready" > /dev/null 2>&1; then
            log_success "Controller ready after ${elapsed}s"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    log_error "Controller did not become ready within ${RESTART_TIMEOUT}s"
    return 1
}

# =============================================================================
# Fixture + rv probe helpers
#
# Authentication is handled by `ensure_admin_token` and passed via the Bearer
# header. `curl -s` is used (NOT -sf) so we can inspect status codes without
# bash exiting via `set -e` / pipefail; the helpers return empty rv on error,
# and the caller asserts on the numeric form.
# =============================================================================
write_fixture() {
    cat > "${FIXTURE_PATH}" <<YAML
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: ${FIXTURE_NAME}
  namespace: ${FIXTURE_NS}
spec:
  parentRefs:
    - name: http-test-gateway
      namespace: ${FIXTURE_NS}
  hostnames:
    - "rv-probe.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /probe
      backendRefs:
        - name: rv-probe-backend
          port: 8080
YAML
}

# admin_write VERB PATH PAYLOAD -> echoes "HTTP_CODE BODY"
admin_write() {
    local verb=$1
    local path=$2
    local payload=${3:-}
    ensure_admin_token
    local response_body http_code
    # `--write-out '\n%{http_code}'` keeps body and code in one stream; we then
    # split. We deliberately do NOT use `-f` so we can inspect non-2xx codes.
    response_body=$(curl -s -o - -w '\n__HTTP_CODE__%{http_code}__' \
        -X "${verb}" \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/yaml" \
        ${payload:+--data-binary "${payload}"} \
        "${CONTROLLER_URL}${path}" 2>/dev/null || echo $'\n__HTTP_CODE__000__')
    http_code=$(echo "$response_body" | sed -n 's/.*__HTTP_CODE__\([0-9][0-9][0-9]\)__.*/\1/p' | tail -1)
    echo "${http_code:-000}"
}

# admin_get_rv -> echoes the resource's metadata.resourceVersion (or empty on error)
admin_get_rv() {
    ensure_admin_token
    local body
    body=$(curl -s -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${CONTROLLER_URL}/api/v1/namespaced/HTTPRoute/${FIXTURE_NS}/${FIXTURE_NAME}" 2>/dev/null || echo "")
    if [ -z "$body" ]; then
        echo ""
        return
    fi
    # `get_namespaced` returns the raw resource JSON directly (no ApiResponse
    # envelope). Index straight in -- if a future change wraps the response
    # under `{data: ...}`, this test should fail loudly rather than silently
    # mask a contract regression.
    echo "$body" | python3 -c '
import json, sys
try:
    doc = json.load(sys.stdin)
except Exception:
    print("")
    sys.exit(0)
if not isinstance(doc, dict):
    print("")
    sys.exit(0)
print(doc.get("metadata", {}).get("resourceVersion", ""))
' 2>/dev/null || echo ""
}

create_or_skip() {
    local payload
    payload=$(cat "${FIXTURE_PATH}")
    local code
    code=$(admin_write POST "/api/v1/namespaced/HTTPRoute/${FIXTURE_NS}" "${payload}")
    # POST-create returns 201 Created per RFC 9110 §15.3.2.
    if [ "$code" != "201" ]; then
        log_error "POST returned HTTP ${code}; expected 201"
        return 1
    fi
    return 0
}

update_route() {
    local payload
    payload=$(cat "${FIXTURE_PATH}")
    local code
    code=$(admin_write PUT "/api/v1/namespaced/HTTPRoute/${FIXTURE_NS}/${FIXTURE_NAME}" "${payload}")
    if [ "$code" != "200" ]; then
        log_error "PUT returned HTTP ${code}; expected 200"
        return 1
    fi
    return 0
}

delete_route_if_exists() {
    if [ -n "$ADMIN_TOKEN" ]; then
        local code
        code=$(admin_write DELETE "/api/v1/namespaced/HTTPRoute/${FIXTURE_NS}/${FIXTURE_NAME}" "")
        # DELETE returns 204 No Content per RFC 9110 §9.3.5; 404 is also fine
        # because the cleanup is best-effort (the route may not exist yet).
        if [ "$code" != "204" ] && [ "$code" != "404" ]; then
            log_error "DELETE returned HTTP ${code}; expected 204 or 404"
        fi
    fi
}

assert_strictly_greater() {
    local name=$1
    local newer=$2
    local older=$3
    if ! [[ "$newer" =~ ^[0-9]+$ ]] || ! [[ "$older" =~ ^[0-9]+$ ]]; then
        assert_pass "$name" "fail" "non-numeric rv: newer='${newer}' older='${older}'"
        return
    fi
    if [ "$newer" -gt "$older" ]; then
        assert_pass "$name" "pass"
    else
        assert_pass "$name" "fail" "expected newer(${newer}) > older(${older})"
    fi
}

# =============================================================================
# Main
# =============================================================================
main() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --no-start)    DO_START=false;   shift ;;
            --keep-alive)  DO_CLEANUP=false; shift ;;
            -h|--help)
                echo "Usage: $0 [--no-start] [--keep-alive]"
                echo "  --no-start    Attach to an already-running environment"
                echo "  --keep-alive  Do not tear down the environment on exit"
                exit 0
                ;;
            *) log_error "Unknown option: $1"; exit 1 ;;
        esac
    done

    trap cleanup EXIT

    log_section "Resource Version Restart Monotonicity Test"

    if $DO_START; then
        start_environment
    else
        log_info "Attaching to existing environment (--no-start)"
    fi

    resolve_work_dir
    check_prerequisites
    write_fixture
    ensure_admin_token

    # Remove any stale fixture from a previous interrupted run.
    delete_route_if_exists

    # -------------------------------------------------------------------------
    # Phase 1: in-session monotonicity (smoke test on the wiring)
    # -------------------------------------------------------------------------
    log_section "Phase 1: in-session monotonicity"
    if ! create_or_skip; then
        assert_pass "phase1_create" "fail" "POST failed"
        finalize
        return
    fi
    local rv1
    rv1=$(admin_get_rv)
    log_info "rv after POST: ${rv1:-<empty>}"

    if ! update_route; then
        assert_pass "phase1_update" "fail" "PUT failed"
        finalize
        return
    fi
    local rv2
    rv2=$(admin_get_rv)
    log_info "rv after PUT: ${rv2:-<empty>}"
    assert_strictly_greater "rv2_greater_than_rv1" "$rv2" "$rv1"

    # -------------------------------------------------------------------------
    # Phase 2: cross-restart monotonicity (the real check)
    # -------------------------------------------------------------------------
    log_section "Phase 2: cross-restart monotonicity"
    if ! restart_controller; then
        assert_pass "controller_restart_ready" "fail" "/ready did not come back"
        finalize
        return
    fi

    # /ready was the readiness signal; PUT through the gated write route now.
    if ! update_route; then
        assert_pass "phase2_update" "fail" "PUT after restart failed"
        finalize
        return
    fi
    local rv_after
    rv_after=$(admin_get_rv)
    log_info "rv after restart + PUT: ${rv_after:-<empty>}"
    assert_strictly_greater "rv_after_greater_than_rv2" "$rv_after" "$rv2"

    finalize
}

finalize() {
    delete_route_if_exists
    log_section "Summary"
    log_info "PASSED: ${PASSED}"
    log_info "FAILED: ${FAILED}"
    if [ "$FAILED" -gt 0 ]; then
        exit 1
    fi
}

main "$@"
