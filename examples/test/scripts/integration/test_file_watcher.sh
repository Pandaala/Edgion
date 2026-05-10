#!/bin/bash
# =============================================================================
# FileWatcher integration test (FileSystem backend)
#
# Drops YAML files into the controller's conf directory and verifies the
# runtime PollWatcher picks them up via the controller Admin API.
#
# Two contracts the test must respect (otherwise events get filtered out):
#   1. Filenames MUST follow the canonical form expected by
#      `parse_resource_filename` in `controller/conf_mgr/conf_center/
#      file_system/file_watcher.rs`:
#        - namespaced:     {Kind}_{namespace}_{name}.yaml
#        - cluster-scoped: {Kind}__{name}.yaml (double underscore)
#      Files that don't match (e.g. `gateway.yaml`) are silently dropped on
#      Apply and Delete dispatch — required because Delete events have no
#      file content to read, only the path.
#   2. Cluster-scoped queries go through `/api/v1/cluster/{kind}`; namespaced
#      queries through `/api/v1/namespaced/{kind}`. The router has separate
#      handlers — using the wrong scope returns count=0 even when the
#      resource exists.
#
# Wait budget per step is 3s: PollWatcher's poll_interval is 1s, the runtime
# loop dispatches events synchronously, and resource processing adds a few
# hundred ms. Two seconds was tight enough to flake on busy machines.
# =============================================================================

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/file_watcher_test_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
STAGING_DIR="${WORK_DIR}/staging"   # outside CONFIG_DIR, used by mv-in/mv-out tests

CONTROLLER_ADMIN_PORT=15800
WAIT_SECS=3

CONTROLLER_PID=""

# =============================================================================
# Logging
# =============================================================================
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error()   { echo -e "${RED}[✗]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# =============================================================================
# Cleanup
# =============================================================================
cleanup() {
    log_section "Cleanup"

    if [ -n "$CONTROLLER_PID" ] && kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_info "Stopping edgion-controller (PID: $CONTROLLER_PID)"
        kill "$CONTROLLER_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$CONTROLLER_PID" 2>/dev/null || true
    fi

    if [ -d "$WORK_DIR" ]; then
        log_info "Work dir preserved (delete manually if needed): $WORK_DIR"
    fi
}

trap cleanup EXIT

# =============================================================================
# Setup
# =============================================================================
init_test_env() {
    log_section "Init test environment"

    mkdir -p "$CONFIG_DIR" "$LOG_DIR" "$PID_DIR" "$STAGING_DIR"

    # Controller in non-K8s mode requires CRD schemas under <conf-dir>/crd
    # or it FATAL-exits with "No schemas loaded in non-K8s mode".
    local crd_src="${PROJECT_ROOT}/config/crd"
    if [ ! -d "$crd_src" ]; then
        log_error "CRD schema source not found: ${crd_src}"
        exit 1
    fi
    mkdir -p "${CONFIG_DIR}/crd"
    cp -R "${crd_src}/." "${CONFIG_DIR}/crd/"
    log_info "Copied CRD schemas into ${CONFIG_DIR}/crd"

    log_info "Work dir:    $WORK_DIR"
    log_info "Config dir:  $CONFIG_DIR"
    log_info "Log dir:     $LOG_DIR"
    log_info "Staging dir: $STAGING_DIR (outside watched config)"
}

# =============================================================================
# Start controller
# =============================================================================
start_controller() {
    log_section "Starting edgion-controller (FileSystem backend)"

    local controller_bin="${PROJECT_ROOT}/target/debug/edgion-controller"
    if [ ! -f "$controller_bin" ]; then
        log_error "Controller binary not found: $controller_bin"
        log_info "Run cargo build first."
        exit 1
    fi

    "$controller_bin" \
        --work-dir "$WORK_DIR" \
        --log-dir "$LOG_DIR" \
        --conf-dir "$CONFIG_DIR" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        > "$LOG_DIR/controller.log" 2>&1 &

    CONTROLLER_PID=$!
    echo "$CONTROLLER_PID" > "$PID_DIR/controller.pid"
    log_info "Controller PID: $CONTROLLER_PID"

    log_info "Waiting for controller /health..."
    local waited=0
    while [ $waited -lt 30 ]; do
        if curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/health" > /dev/null 2>&1; then
            log_success "Controller ready"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    log_error "Controller did not become ready within 30s"
    cat "$LOG_DIR/controller.log"
    exit 1
}

# =============================================================================
# Auth helper — Controller in non-K8s mode auto-generates an admin Secret
# under <conf-dir>; we read it once, login, cache the bearer token, and
# inject it into every admin-API curl.
# =============================================================================
ADMIN_TOKEN=""
ensure_admin_token() {
    if [ -n "$ADMIN_TOKEN" ]; then
        return 0
    fi
    local secret_file="${CONFIG_DIR}/Secret_edgion-system_edgion-admin-credentials.yaml"
    local waited=0
    while [ $waited -lt 10 ] && [ ! -f "$secret_file" ]; do
        sleep 1
        waited=$((waited + 1))
    done
    if [ ! -f "$secret_file" ]; then
        log_error "Admin Secret was not generated at $secret_file"
        exit 1
    fi
    local pass
    pass=$(awk '/^[[:space:]]+password:/ {print $2; exit}' "$secret_file" | tr -d '"')
    ADMIN_TOKEN=$(curl -s -H "Content-Type: application/json" \
        -d "{\"username\":\"admin\",\"password\":\"$pass\"}" \
        "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/auth/login" \
        | python3 -c "import sys,json; print(json.load(sys.stdin).get('data',{}).get('token',''))")
    if [ -z "$ADMIN_TOKEN" ]; then
        log_error "login failed: could not obtain admin token"
        exit 1
    fi
}

curl_admin() {
    ensure_admin_token
    curl -s -H "Authorization: Bearer $ADMIN_TOKEN" "$@"
}

# =============================================================================
# Test cases
# =============================================================================

# Test 1: cluster-scoped Add — GatewayClass via /api/v1/cluster/...
test_add_gateway_class() {
    log_section "Test 1: Add cluster-scoped resource (GatewayClass)"

    cat > "$CONFIG_DIR/GatewayClass__edgion.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
EOF

    log_info "Wrote GatewayClass__edgion.yaml; waiting ${WAIT_SECS}s for PollWatcher cycle..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/cluster/gatewayclass")
    if echo "$response" | grep -q "edgion"; then
        log_success "GatewayClass 'edgion' visible via /api/v1/cluster/gatewayclass"
    else
        log_error "GatewayClass not picked up"
        echo "Response: $response"
        return 1
    fi
}

# Test 2: namespaced Add — Gateway via /api/v1/namespaced/...
test_add_gateway() {
    log_section "Test 2: Add namespaced resource (Gateway)"

    cat > "$CONFIG_DIR/Gateway_default_test-gateway.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-gateway
  namespace: default
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 8080
      protocol: HTTP
EOF

    log_info "Wrote Gateway_default_test-gateway.yaml; waiting ${WAIT_SECS}s..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    if echo "$response" | grep -q "test-gateway"; then
        log_success "Gateway 'test-gateway' visible"
    else
        log_error "Gateway not picked up"
        echo "Response: $response"
        return 1
    fi
}

# Test 3: Modify (rewrite same canonical filename) — port 8080 -> 9090
test_modify_gateway() {
    log_section "Test 3: Modify existing Gateway (port 8080 -> 9090)"

    cat > "$CONFIG_DIR/Gateway_default_test-gateway.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-gateway
  namespace: default
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 9090
      protocol: HTTP
EOF

    log_info "Rewrote Gateway with port 9090; waiting ${WAIT_SECS}s..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    if echo "$response" | grep -q '"port":9090\|port: 9090\|9090'; then
        log_success "Gateway port updated to 9090"
    else
        log_error "Gateway port did not update"
        echo "Response: $response"
        return 1
    fi
}

# Test 4: Add HTTPRoute
test_add_http_route() {
    log_section "Test 4: Add HTTPRoute"

    cat > "$CONFIG_DIR/HTTPRoute_default_test-route.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: test-route
  namespace: default
spec:
  parentRefs:
    - name: test-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      backendRefs:
        - name: backend-svc
          port: 8080
EOF

    log_info "Wrote HTTPRoute_default_test-route.yaml; waiting ${WAIT_SECS}s..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/httproute")
    if echo "$response" | grep -q "test-route"; then
        log_success "HTTPRoute 'test-route' visible"
    else
        log_error "HTTPRoute not picked up"
        echo "Response: $response"
        return 1
    fi
}

# Test 5: Rapid back-to-back modifications coalesce to the latest state.
# This is exactly the scenario that motivated `with_compare_contents(true)`:
# atomic-replace writes inside one poll interval must not be silently
# elided by the diff layer.
test_rapid_modifications() {
    log_section "Test 5: Rapid modifications (5 ports in quick succession)"

    for i in 1 2 3 4 5; do
        cat > "$CONFIG_DIR/Gateway_default_test-gateway.yaml" << EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-gateway
  namespace: default
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: $((9000 + i))
      protocol: HTTP
EOF
        log_info "Rapid write #${i}: port=$((9000 + i))"
        sleep 0.1
    done

    log_info "Waiting ${WAIT_SECS}s for poll + processing..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    if echo "$response" | grep -q "9005"; then
        log_success "Final port 9005 reflected"
    else
        log_error "Final port not reflected"
        echo "Response: $response"
        return 1
    fi
}

# Test 6: Move file IN (rename from outside the watched dir into it)
test_move_file_in() {
    log_section "Test 6: Move file into watched dir"

    cat > "$STAGING_DIR/Service_default_moved-svc.yaml" << 'EOF'
apiVersion: v1
kind: Service
metadata:
  name: moved-svc
  namespace: default
spec:
  ports:
    - port: 8080
      targetPort: 8080
  selector:
    app: backend
EOF

    log_info "Staged Service_default_moved-svc.yaml; mv into config dir"
    mv "$STAGING_DIR/Service_default_moved-svc.yaml" "$CONFIG_DIR/Service_default_moved-svc.yaml"

    log_info "Waiting ${WAIT_SECS}s..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/service")
    if echo "$response" | grep -q "moved-svc"; then
        log_success "Service 'moved-svc' visible after move-in"
    else
        log_error "Service not picked up after move-in"
        echo "Response: $response"
        return 1
    fi
}

# Test 7: Move file OUT (rename from watched dir out of it)
test_move_file_out() {
    log_section "Test 7: Move file out of watched dir"

    mv "$CONFIG_DIR/Service_default_moved-svc.yaml" "$STAGING_DIR/Service_default_moved-svc.yaml"
    log_info "Moved Service_default_moved-svc.yaml out; waiting ${WAIT_SECS}s..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/service")
    if echo "$response" | grep -q "moved-svc"; then
        log_error "Service still visible after move-out"
        echo "Response: $response"
        return 1
    else
        log_success "Service removed after move-out"
    fi
}

# Test 8: Delete file → resource removed
test_delete_file() {
    log_section "Test 8: Delete file"

    rm -f "$CONFIG_DIR/HTTPRoute_default_test-route.yaml"
    log_info "Deleted HTTPRoute_default_test-route.yaml; waiting ${WAIT_SECS}s..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/httproute")
    if echo "$response" | grep -q "test-route"; then
        log_error "HTTPRoute still visible after delete"
        echo "Response: $response"
        return 1
    else
        log_success "HTTPRoute removed after delete"
    fi
}

# =============================================================================
# Main
# =============================================================================
main() {
    log_section "FileWatcher integration test"
    log_info "Project root: $PROJECT_ROOT"

    init_test_env
    start_controller

    local failed=0

    test_add_gateway_class    || failed=1
    test_add_gateway          || failed=1
    test_modify_gateway       || failed=1
    test_add_http_route       || failed=1
    test_rapid_modifications  || failed=1
    test_move_file_in         || failed=1
    test_move_file_out        || failed=1
    test_delete_file          || failed=1

    log_section "Summary"
    if [ $failed -eq 0 ]; then
        log_success "All FileWatcher tests passed"
        log_info "Controller log: $LOG_DIR/controller.log"
        return 0
    else
        log_error "Some FileWatcher tests failed"
        log_info "Controller log: cat $LOG_DIR/controller.log"
        return 1
    fi
}

main "$@"
