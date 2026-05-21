#!/bin/bash
# =============================================================================
# conf_sync gRPC mTLS Integration Test
#
# Self-contained end-to-end test that verifies:
#   1. edgion-cli gen-certs produces valid mTLS certificates
#   2. Controller starts gRPC listener with server TLS + client CA verification
#   3. Gateway connects to Controller via mTLS client cert
#   4. Configuration synchronisation works correctly over the mTLS channel
#   5. Resource diff between Controller and Gateway passes
#
# Usage:
#   ./run_conf_sync_mtls_test.sh                   # Full run
#   ./run_conf_sync_mtls_test.sh --no-start        # Attach to running env
#   ./run_conf_sync_mtls_test.sh --keep-alive      # Don't stop after test
# =============================================================================

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UTILS_DIR="${SCRIPT_DIR}/../utils"
CERTS_DIR_SCRIPT="${SCRIPT_DIR}/../gen_certs"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
EDGION_CLI="${PROJECT_ROOT}/target/debug/edgion-cli"

# Ports — use different gRPC port to avoid conflict with plain-text tests
CONTROLLER_GRPC_PORT="${EDGION_TEST_MTLS_GRPC_PORT:-50061}"
CONTROLLER_ADMIN_PORT=5800
GATEWAY_ADMIN_PORT=5900
TEST_SERVER_HTTP_PORT=30001

# Configurable
DO_START=true
DO_CLEANUP=true

PASSED=0
FAILED=0
TOTAL=0

# Work directory (set after start)
WORK_DIR=""

# =============================================================================
# Log helpers
# =============================================================================
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error()   { echo -e "${RED}[✗]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[!]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# =============================================================================
# Argument parsing
# =============================================================================
while [[ $# -gt 0 ]]; do
    case $1 in
        --no-start)    DO_START=false; DO_CLEANUP=false; shift ;;
        --keep-alive)  DO_CLEANUP=false; shift ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --no-start      Attach to already-running mTLS environment"
            echo "  --keep-alive    Don't stop services after test"
            echo "  -h, --help      Show help"
            exit 0
            ;;
        *) log_error "Unknown option: $1"; exit 1 ;;
    esac
done

# =============================================================================
# Cleanup
# =============================================================================
cleanup() {
    if $DO_CLEANUP; then
        log_section "Stopping all processes"
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    fi
}
trap cleanup EXIT

# =============================================================================
# Utility: wait for port
# =============================================================================
wait_for_port() {
    local port=$1 label=$2 pid=$3 timeout=${4:-30} elapsed=0
    log_info "Waiting for $label (port $port)..."
    while [ $elapsed -lt $timeout ]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            log_error "$label process exited (PID $pid)"
            return 1
        fi
        if nc -z 127.0.0.1 "$port" 2>/dev/null; then
            log_success "$label port ready ($port)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    log_error "$label did not become ready within ${timeout}s"
    return 1
}

wait_for_health() {
    local url=$1 label=$2 timeout=${3:-30} elapsed=0
    log_info "Health-checking $label..."
    while [ $elapsed -lt $timeout ]; do
        if curl -sf "$url" >/dev/null 2>&1; then
            log_success "$label healthy"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    log_error "$label health check failed after ${timeout}s"
    return 1
}

wait_for_ready() {
    local url=$1 label=$2 timeout=${3:-30} elapsed=0
    log_info "Waiting for $label readiness..."
    while [ $elapsed -lt $timeout ]; do
        if curl -sf "$url" >/dev/null 2>&1; then
            log_success "$label ready"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    log_error "$label not ready after ${timeout}s"
    return 1
}

# =============================================================================
# Test assertion
# =============================================================================
assert_ok() {
    local name=$1
    shift
    TOTAL=$((TOTAL + 1))
    if "$@"; then
        PASSED=$((PASSED + 1))
        log_success "PASS: $name"
    else
        FAILED=$((FAILED + 1))
        log_error "FAIL: $name"
    fi
}

check_port_conflict() {
    local port=$1 label=$2
    if command -v lsof >/dev/null 2>&1; then
        local usage
        usage=$(lsof -n -P -iTCP:"${port}" -sTCP:LISTEN 2>/dev/null || true)
        if [ -n "$usage" ]; then
            log_error "$label port $port occupied"
            echo "$usage"
            return 1
        fi
    elif nc -z 127.0.0.1 "$port" 2>/dev/null; then
        log_error "$label port $port occupied"
        return 1
    fi
    return 0
}

# =============================================================================
# Start environment with mTLS
# =============================================================================
start_mtls_environment() {
    log_section "Preparing mTLS test environment"

    # ── 0. Kill old processes ──
    log_info "Killing old processes..."
    pkill -9 -f "edgion-controller" 2>/dev/null || true
    pkill -9 -f "edgion-gateway" 2>/dev/null || true
    pkill -9 -f "test_server" 2>/dev/null || true
    sleep 2

    # ── 1. Check binaries ──
    log_section "Checking binaries"
    for bin in edgion-controller edgion-gateway edgion-cli; do
        if [ ! -f "${PROJECT_ROOT}/target/debug/${bin}" ]; then
            log_error "${bin} not found — run prepare.sh first"
            exit 1
        fi
    done
    for bin in test_server resource_diff; do
        if [ ! -f "${PROJECT_ROOT}/target/debug/examples/${bin}" ]; then
            log_error "${bin} not found — run prepare.sh first"
            exit 1
        fi
    done
    log_success "All binaries present"

    # ── 2. Work directory ──
    local timestamp
    timestamp=$(date +%Y%m%d_%H%M%S)
    WORK_DIR="${PROJECT_ROOT}/integration_testing/mtls_testing_${timestamp}"
    local log_dir="${WORK_DIR}/logs"
    local pid_dir="${WORK_DIR}/pids"
    local config_dir="${WORK_DIR}/config"
    local certs_dir="${WORK_DIR}/certs/grpc"
    local generated_secret_dir="${WORK_DIR}/generated-secrets"
    mkdir -p "$log_dir" "$pid_dir" "$config_dir" "$certs_dir" "$generated_secret_dir"

    export EDGION_WORK_DIR="$WORK_DIR"
    export EDGION_GENERATED_SECRET_DIR="$generated_secret_dir"

    # Copy CRD schemas
    if [ -d "${PROJECT_ROOT}/config/crd" ]; then
        cp -r "${PROJECT_ROOT}/config/crd" "$config_dir/"
        log_success "CRD schemas copied"
    fi

    # ── 3. Generate conf_sync mTLS certificates ──
    log_section "Generating conf_sync mTLS certificates"
    EDGION_CONF_SYNC_CERTS_DIR="$certs_dir" \
        bash "${CERTS_DIR_SCRIPT}/generate_conf_sync_certs.sh" --force

    # New multi-cert layout: files are under primary/ and secondary/ subdirectories
    for set_name in primary secondary; do
        for f in ca.crt server.crt server.key client.crt client.key; do
            if [ ! -f "$certs_dir/$set_name/$f" ]; then
                log_error "Missing expected cert file: $certs_dir/$set_name/$f"
                exit 1
            fi
        done
    done
    log_success "All mTLS certificates present (primary + secondary)"

    # ── 4. Generate YAML configs with multi-cert mTLS section ──
    log_section "Creating mTLS-enabled YAML configs (multi-cert format)"

    local controller_yaml="${WORK_DIR}/edgion-controller-mtls.yaml"
    local gateway_yaml="${WORK_DIR}/edgion-gateway-mtls.yaml"

    # Controller YAML: base + multi-cert mTLS override
    # active: primary selects the primary cert; secondary is listed but inactive
    cp "${PROJECT_ROOT}/config/edgion-controller.yaml" "$controller_yaml"
    cat >> "$controller_yaml" <<EOF

# ── Multi-cert mTLS override for integration test ──
conf_sync_security:
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/primary/server.crt
      key: certs/grpc/primary/server.key
      ca: certs/grpc/primary/ca.crt
    - name: secondary
      cert: certs/grpc/secondary/server.crt
      key: certs/grpc/secondary/server.key
      ca: certs/grpc/secondary/ca.crt

# Disable admin auth for this conf_sync test (it verifies conf_sync, not auth).
# With local_auth.enabled=false, derive_require_auth() returns false (no-auth
# mode), so /api/v1/server-info is reachable without a Bearer token.
local_auth:
  enabled: false
EOF
    log_success "Controller YAML (multi-cert): $controller_yaml"

    # Gateway YAML: base + multi-cert mTLS override
    # active: primary selects the primary cert; secondary is listed but inactive
    cp "${PROJECT_ROOT}/config/edgion-gateway.yaml" "$gateway_yaml"
    cat >> "$gateway_yaml" <<EOF

# ── Multi-cert mTLS override for integration test ──
conf_sync_security:
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/primary/client.crt
      key: certs/grpc/primary/client.key
      ca: certs/grpc/primary/ca.crt
    - name: secondary
      cert: certs/grpc/secondary/client.crt
      key: certs/grpc/secondary/client.key
      ca: certs/grpc/secondary/ca.crt
EOF
    log_success "Gateway YAML (multi-cert): $gateway_yaml"

    # ── 4b. Also generate backward-compat YAML configs (old single-cert format) ──
    # These are written out for reference / manual testing, but NOT used in the
    # automated test run (which uses the multi-cert format above).
    local controller_yaml_compat="${WORK_DIR}/edgion-controller-mtls-compat.yaml"
    local gateway_yaml_compat="${WORK_DIR}/edgion-gateway-mtls-compat.yaml"

    cp "${PROJECT_ROOT}/config/edgion-controller.yaml" "$controller_yaml_compat"
    cat >> "$controller_yaml_compat" <<EOF

# ── Backward-compat single-cert mTLS (old format, still supported) ──
conf_sync_security:
  tls:
    cert: certs/grpc/primary/server.crt
    key: certs/grpc/primary/server.key
    ca: certs/grpc/primary/ca.crt
    skip_tls: false
EOF

    cp "${PROJECT_ROOT}/config/edgion-gateway.yaml" "$gateway_yaml_compat"
    cat >> "$gateway_yaml_compat" <<EOF

# ── Backward-compat single-cert mTLS (old format, still supported) ──
conf_sync_security:
  tls:
    cert: certs/grpc/primary/client.crt
    key: certs/grpc/primary/client.key
    ca: certs/grpc/primary/ca.crt
    skip_tls: false
EOF
    log_success "Backward-compat YAML configs written (not used in this run)"

    # ── 5. Generate runtime TLS/mTLS certs for test suites ──
    log_section "Generating test suite certificates"

    if [ -f "${CERTS_DIR_SCRIPT}/generate_tls_certs.sh" ]; then
        EDGION_GENERATED_SECRET_DIR="$generated_secret_dir" \
            bash "${CERTS_DIR_SCRIPT}/generate_tls_certs.sh" > /dev/null 2>&1 || true
    fi
    if [ -f "${CERTS_DIR_SCRIPT}/generate_backend_certs.sh" ]; then
        EDGION_GENERATED_SECRET_DIR="$generated_secret_dir" \
            bash "${CERTS_DIR_SCRIPT}/generate_backend_certs.sh" > /dev/null 2>&1 || true
    fi
    if [ -f "${CERTS_DIR_SCRIPT}/generate_mtls_certs.sh" ]; then
        EDGION_GENERATED_SECRET_DIR="$generated_secret_dir" \
            bash "${CERTS_DIR_SCRIPT}/generate_mtls_certs.sh" > /dev/null 2>&1 || true
    fi
    log_success "Test suite certificates generated"

    # ── 6. Start test_server ──
    log_section "Starting test_server"
    local backend_cert="${PROJECT_ROOT}/examples/test/certs/backend/server.crt"
    local backend_key="${PROJECT_ROOT}/examples/test/certs/backend/server.key"
    local backend_mtls_ca="${PROJECT_ROOT}/examples/test/certs/mtls/client-ca.crt"

    local https_args=""
    if [ -f "$backend_cert" ] && [ -f "$backend_key" ]; then
        https_args="--https-backend-port 30051 --cert-file $backend_cert --key-file $backend_key"
    fi
    local mtls_args=""
    if [ -f "$backend_cert" ] && [ -f "$backend_key" ] && [ -f "$backend_mtls_ca" ]; then
        mtls_args="--https-backend-mtls-port 30052 --client-ca-file $backend_mtls_ca"
    fi

    "${PROJECT_ROOT}/target/debug/examples/test_server" \
        --http-ports "30001,30002,30003" \
        --grpc-ports "30021,30022,30023" \
        --websocket-port 30005 \
        --tcp-port 30010 \
        --udp-port 30011 \
        --tcp-pp2-port 30012 \
        --auth-port 30040 \
        --log-level info \
        $https_args \
        $mtls_args \
        > "${log_dir}/test_server.log" 2>&1 &
    local ts_pid=$!
    echo $ts_pid > "${pid_dir}/test_server.pid"

    if ! wait_for_port $TEST_SERVER_HTTP_PORT "test_server" $ts_pid 30; then
        log_error "test_server failed to start. Log:"
        tail -20 "${log_dir}/test_server.log" 2>/dev/null || true
        exit 1
    fi
    wait_for_health "http://127.0.0.1:${TEST_SERVER_HTTP_PORT}/health" "test_server" 10

    # ── 7. Start controller with mTLS ──
    log_section "Starting edgion-controller (mTLS)"

    if ! check_port_conflict "$CONTROLLER_GRPC_PORT" "controller gRPC"; then
        exit 1
    fi

    "${PROJECT_ROOT}/target/debug/edgion-controller" \
        -c "$controller_yaml" \
        --work-dir "${WORK_DIR}" \
        --conf-dir "$config_dir" \
        --grpc-listen "0.0.0.0:${CONTROLLER_GRPC_PORT}" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        --test-mode \
        > "${log_dir}/controller.log" 2>&1 &
    local ctrl_pid=$!
    echo $ctrl_pid > "${pid_dir}/controller.pid"

    if ! wait_for_port $CONTROLLER_ADMIN_PORT "controller admin" $ctrl_pid 30; then
        log_error "Controller failed to start. Log:"
        tail -30 "${log_dir}/controller.log" 2>/dev/null || true
        exit 1
    fi
    wait_for_health "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/health" "controller" 10

    if ! wait_for_port $CONTROLLER_GRPC_PORT "controller gRPC (mTLS)" $ctrl_pid 30; then
        log_error "Controller gRPC (mTLS) port failed. Log:"
        tail -30 "${log_dir}/controller.log" 2>/dev/null || true
        exit 1
    fi

    # Verify mTLS is enabled in controller logs
    if grep -q "conf_sync mTLS enabled for gRPC server" "${log_dir}/controller.log" 2>/dev/null; then
        log_success "Controller gRPC mTLS confirmed via log"
    else
        log_warn "Could not confirm mTLS log entry (may appear later)"
    fi

    # ── 8. Wait for ConfigServer ready, load base config ──
    wait_for_ready "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/ready" "controller ConfigServer" 30

    log_section "Loading base configuration via Admin API"
    local conf_src="${PROJECT_ROOT}/examples/test/conf/base"
    if [ -d "$conf_src" ]; then
        for file in "$conf_src"/*.yaml; do
            [ -f "$file" ] || continue
            "$EDGION_CLI" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" apply -f "$file" > /dev/null 2>&1 || true
        done
        log_success "Base config loaded"
    fi

    # Load a simple test suite (HTTPRoute/Basic)
    local load_script="${UTILS_DIR}/load_conf.sh"
    if [ -f "$load_script" ]; then
        bash "$load_script" --wait 0 "HTTPRoute/Basic" > /dev/null 2>&1 || true
        log_success "HTTPRoute/Basic suite loaded"
    fi

    # Load generated secrets
    if [ -d "$generated_secret_dir" ]; then
        local secret_files
        secret_files=$(find "$generated_secret_dir" -type f \( -name "*.yaml" -o -name "*.yml" \) 2>/dev/null | sort)
        for file in $secret_files; do
            "$EDGION_CLI" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" apply -f "$file" > /dev/null 2>&1 || true
        done
        log_success "Generated secrets loaded"
    fi

    # ── 9. Start gateway with mTLS ──
    log_section "Starting edgion-gateway (mTLS)"
    local gw_rust_log="info,pingora_proxy=error,pingora_core=error"

    EDGION_ACCESS_LOG="${log_dir}/edgion_access.log" \
    RUST_LOG="${gw_rust_log}" \
    EDGION_TEST_ACCESS_LOG_PATH="${log_dir}/edgion_access.log" \
    "${PROJECT_ROOT}/target/debug/edgion-gateway" \
        -c "$gateway_yaml" \
        --work-dir "${WORK_DIR}" \
        --server-addr "https://127.0.0.1:${CONTROLLER_GRPC_PORT}" \
        --integration-testing-mode \
        > "${log_dir}/gateway.log" 2>&1 &
    local gw_pid=$!
    echo $gw_pid > "${pid_dir}/gateway.pid"

    if ! wait_for_port $GATEWAY_ADMIN_PORT "gateway admin" $gw_pid 30; then
        log_error "Gateway failed to start. Log:"
        tail -30 "${log_dir}/gateway.log" 2>/dev/null || true
        exit 1
    fi

    if ! wait_for_ready "http://127.0.0.1:${GATEWAY_ADMIN_PORT}/ready" "gateway" 60; then
        log_error "Gateway not ready. Log:"
        tail -50 "${log_dir}/gateway.log" 2>/dev/null || true
        exit 1
    fi

    # Wait for LB preload
    local lb_timeout=15 lb_waited=0
    while ! grep -q "LB preload completed" "${log_dir}/gateway.log" 2>/dev/null; do
        if [ $lb_waited -ge $lb_timeout ]; then
            log_warn "LB preload timeout after ${lb_timeout}s (continuing anyway)"
            break
        fi
        sleep 1
        ((lb_waited++))
    done

    # Verify mTLS in gateway logs
    if grep -q "conf_sync mTLS enabled for gRPC client" "${log_dir}/gateway.log" 2>/dev/null; then
        log_success "Gateway gRPC mTLS confirmed via log"
    else
        log_warn "Could not confirm Gateway mTLS log entry"
    fi

    log_success "mTLS test environment started"
    log_info "Work directory: ${WORK_DIR}"
    log_info "Controller: PID $(cat "${pid_dir}/controller.pid"), admin http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
    log_info "Gateway:    PID $(cat "${pid_dir}/gateway.pid"), admin http://127.0.0.1:${GATEWAY_ADMIN_PORT}"
    log_info "gRPC mTLS:  https://127.0.0.1:${CONTROLLER_GRPC_PORT}"

    # Save .current pointer
    mkdir -p "${PROJECT_ROOT}/integration_testing"
    echo "$WORK_DIR" > "${PROJECT_ROOT}/integration_testing/.current"
}

# =============================================================================
# Test: verify mTLS log entries
# =============================================================================
test_mtls_log_entries() {
    local log_dir="${WORK_DIR}/logs"
    local ok=true

    if ! grep -q "conf_sync mTLS enabled for gRPC server" "${log_dir}/controller.log" 2>/dev/null; then
        log_error "Controller log missing mTLS enabled entry"
        ok=false
    fi

    if ! grep -q "conf_sync mTLS enabled for gRPC client" "${log_dir}/gateway.log" 2>/dev/null; then
        log_error "Gateway log missing mTLS enabled entry"
        ok=false
    fi

    # Verify NO skip_tls warnings
    if grep -q "skip_tls=true" "${log_dir}/controller.log" 2>/dev/null; then
        log_error "Controller has skip_tls=true warning — mTLS not truly active"
        ok=false
    fi
    if grep -q "skip_tls=true" "${log_dir}/gateway.log" 2>/dev/null; then
        log_error "Gateway has skip_tls=true warning — mTLS not truly active"
        ok=false
    fi

    $ok
}

# =============================================================================
# Test: Controller and Gateway health
# =============================================================================
test_health() {
    curl -sf "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/health" >/dev/null 2>&1 && \
    curl -sf "http://127.0.0.1:${GATEWAY_ADMIN_PORT}/health" >/dev/null 2>&1
}

# =============================================================================
# Test: Gateway readiness (implies successful mTLS sync)
# =============================================================================
test_gateway_ready() {
    curl -sf "http://127.0.0.1:${GATEWAY_ADMIN_PORT}/ready" >/dev/null 2>&1
}

# =============================================================================
# Test: server_id matches (Gateway got it from Controller via mTLS gRPC)
# =============================================================================
test_server_id_match() {
    local ctrl_id gw_id
    ctrl_id=$(curl -sf "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/api/v1/server-info" 2>/dev/null \
        | grep -o '"server_id":"[^"]*"' | cut -d'"' -f4)
    gw_id=$(curl -sf "http://127.0.0.1:${GATEWAY_ADMIN_PORT}/api/v1/server-info" 2>/dev/null \
        | grep -o '"server_id":"[^"]*"' | cut -d'"' -f4)

    if [ -z "$ctrl_id" ] || [ -z "$gw_id" ]; then
        log_error "Could not fetch server_id (ctrl=$ctrl_id, gw=$gw_id)"
        return 1
    fi

    if [ "$ctrl_id" != "$gw_id" ]; then
        log_error "server_id mismatch: controller=$ctrl_id, gateway=$gw_id"
        return 1
    fi

    log_info "server_id match: $ctrl_id"
    return 0
}

# =============================================================================
# Test: resource_diff passes
# =============================================================================
test_resource_diff() {
    local resource_diff="${PROJECT_ROOT}/target/debug/examples/resource_diff"
    if [ ! -f "$resource_diff" ]; then
        log_warn "resource_diff binary not found, skipping"
        return 0
    fi

    local max_retries=5 delay=2 attempt=1
    while [ $attempt -le $max_retries ]; do
        if "$resource_diff" \
            --controller-url "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" \
            --gateway-url "http://127.0.0.1:${GATEWAY_ADMIN_PORT}" \
            > "${WORK_DIR}/logs/resource_diff_mtls.log" 2>&1; then
            return 0
        fi
        if [ $attempt -lt $max_retries ]; then
            log_info "resource_diff attempt $attempt failed, retrying in ${delay}s..."
            sleep $delay
        fi
        ((attempt++))
    done
    log_error "resource_diff failed after $max_retries attempts"
    tail -15 "${WORK_DIR}/logs/resource_diff_mtls.log" 2>/dev/null || true
    return 1
}

# =============================================================================
# Test: Gateway configclient has resources (verifies gRPC data flow)
# =============================================================================
test_gateway_has_resources() {
    local resp
    resp=$(curl -sf "http://127.0.0.1:${GATEWAY_ADMIN_PORT}/configclient/gateway/list" 2>/dev/null || echo '{"count":0}')
    local count
    count=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('count',0))" 2>/dev/null || echo "0")

    if [ "$count" -gt 0 ]; then
        log_info "Gateway has $count gateway resource(s) synced via mTLS"
        return 0
    else
        log_error "Gateway has no gateway resources — mTLS sync may have failed"
        return 1
    fi
}

# =============================================================================
# Test: plain-text client cannot connect to mTLS server
# =============================================================================
test_plaintext_rejected() {
    # Try connecting to the mTLS gRPC port without TLS.
    # curl returns exit code != 0 on connection/protocol failure, which is what we want.
    if curl -sf --max-time 3 "http://127.0.0.1:${CONTROLLER_GRPC_PORT}/" >/dev/null 2>&1; then
        log_error "Plain-text HTTP request succeeded on mTLS port — TLS may not be active"
        return 1
    fi

    log_info "Plain-text connection correctly rejected by mTLS port"
    return 0
}

# =============================================================================
# Main
# =============================================================================
main() {
    log_section "conf_sync gRPC mTLS Integration Test"

    if $DO_START; then
        start_mtls_environment
    else
        log_info "Attaching to existing environment"
        if [ -f "${PROJECT_ROOT}/integration_testing/.current" ]; then
            WORK_DIR=$(cat "${PROJECT_ROOT}/integration_testing/.current")
            export EDGION_WORK_DIR="$WORK_DIR"
        else
            log_error "No .current work directory found"
            exit 1
        fi
    fi

    # Give a moment for everything to stabilize
    sleep 2

    # ── Run tests ──
    log_section "Running mTLS verification tests"

    assert_ok "mTLS log entries present"          test_mtls_log_entries
    assert_ok "Controller and Gateway healthy"     test_health
    assert_ok "Gateway ready (mTLS sync OK)"       test_gateway_ready
    assert_ok "server_id match via mTLS gRPC"      test_server_id_match
    assert_ok "Gateway has synced resources"        test_gateway_has_resources
    assert_ok "resource_diff passes over mTLS"     test_resource_diff
    assert_ok "Plain-text rejected by mTLS server" test_plaintext_rejected

    # ── Results ──
    log_section "Results"
    echo "  Total:   ${TOTAL}"
    echo "  Passed:  ${PASSED}"
    echo "  Failed:  ${FAILED}"
    echo ""
    if [ -n "$WORK_DIR" ]; then
        echo "  Logs:    ${WORK_DIR}/logs/"
        echo "  Certs:   ${WORK_DIR}/certs/grpc/primary/  (active)"
        echo "           ${WORK_DIR}/certs/grpc/secondary/  (inactive, multi-cert test)"
    fi
    echo ""

    if [ "$FAILED" -gt 0 ]; then
        log_error "${FAILED} test(s) FAILED"
        if [ -n "$WORK_DIR" ]; then
            echo ""
            echo "Controller log tail:"
            tail -20 "${WORK_DIR}/logs/controller.log" 2>/dev/null || true
            echo ""
            echo "Gateway log tail:"
            tail -20 "${WORK_DIR}/logs/gateway.log" 2>/dev/null || true
        fi
        exit 1
    else
        log_success "All ${TOTAL} mTLS tests passed!"
        exit 0
    fi
}

main
