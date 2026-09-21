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
UTILS_DIR="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils"
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/file_watcher_test_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
STAGING_DIR="${WORK_DIR}/staging"   # outside CONFIG_DIR, used by mv-in/mv-out tests

CONTROLLER_ADMIN_PORT=15800
CONTROLLER_PROBE_PORT="${CONTROLLER_PROBE_PORT:-12100}"
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

    # This test otherwise drives the controller purely from CLI flags, but
    # conf_sync_security has no flag and an omitted section is fail-close (the
    # binary refuses to start). Emit a minimal config file carrying ONLY the
    # transport block (mTLS, from the shared emitter); every other setting still
    # comes from the flags below (CLI takes precedence over the config file).
    # The certs are distributed into WORK_DIR, which is also --work-dir, so the
    # emitter's work-dir relative paths resolve.
    #
    # NOTE: this file lives in WORK_DIR, NOT in CONFIG_DIR — CONFIG_DIR is the
    # watched resource directory and a binary config dropped in there would be
    # picked up by the file watcher under test.
    conf_sync_distribute_certs "$WORK_DIR"
    local controller_conf="${WORK_DIR}/edgion-controller.yaml"
    conf_sync_security_block server > "$controller_conf"

    # conf_center.controller_name is mandatory outside Kubernetes and has no CLI
    # flag, so the block has to come from the config file. conf_dir is required
    # by the schema; --conf-dir below resolves to the same directory.
    cat >> "$controller_conf" << EOF

conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: "${CONFIG_DIR}"
EOF

    "$controller_bin" \
        --config-file "$controller_conf" \
        --work-dir "$WORK_DIR" \
        --log-dir "$LOG_DIR" \
        --conf-dir "$CONFIG_DIR" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        --fs-poll-interval-ms 500 \
        > "$LOG_DIR/controller.log" 2>&1 &

    CONTROLLER_PID=$!
    echo "$CONTROLLER_PID" > "$PID_DIR/controller.pid"
    log_info "Controller PID: $CONTROLLER_PID"

    log_info "Waiting for controller /health..."
    local waited=0
    while [ $waited -lt 30 ]; do
        if curl -s "http://localhost:${CONTROLLER_PROBE_PORT}/health" > /dev/null 2>&1; then
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
# Auth helper (token-only model) — the Controller reads a single pre-shared bearer
# token from EDGION_ADMIN_TOKEN (honored by the Controller started below) and
# registers it as the allow-all admin token. We inject that same value into every
# admin-API curl. Honor an inherited value or fall back to a fixed test token.
# =============================================================================
export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
ADMIN_TOKEN=""
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

# Test 9: Nested directory — recursive load
test_nested_dir_load() {
    log_section "Test 9: Load YAML from nested subdirectory"

    mkdir -p "$CONFIG_DIR/gateways"
    cat > "$CONFIG_DIR/gateways/Gateway_default_nested-gw.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: nested-gw
  namespace: default
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 7070
      protocol: HTTP
EOF

    log_info "Wrote gateways/Gateway_default_nested-gw.yaml; waiting ${WAIT_SECS}s..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    if echo "$response" | grep -q "nested-gw"; then
        log_success "Gateway 'nested-gw' loaded from gateways/ subdirectory"
    else
        log_error "nested Gateway not loaded from subdirectory"
        echo "Response: $response"
        return 1
    fi
}

# Test 10: Nested directory — status sidecar follows spec into subdir
test_nested_status_follows_spec() {
    log_section "Test 10: Status sidecar follows spec into subdir"

    local status_file="$CONFIG_DIR/gateways/Gateway_default_nested-gw.yaml.status"
    local waited=0
    while [ $waited -lt 6 ]; do
        if [ -f "$status_file" ]; then
            log_success "Status sidecar at expected nested path: $status_file"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    log_error "Status sidecar did not appear at $status_file"
    log_info "Status files seen under config dir:"
    find "$CONFIG_DIR" -name '*.yaml.status' || true
    return 1
}

# Test 10b: Error sidecar of a statusless resource is cleared once it is repaired
#
# A Secret has no handler-computed status, so a successful reapply persists
# nothing that could overwrite the ParseError sidecar written while the file was
# broken. The controller must delete it explicitly; cleanup_orphans() cannot,
# because the configuration file still exists.
test_error_status_cleared_after_repair() {
    log_section "Test 10b: ParseError sidecar cleared after repairing a Secret"

    local spec_file="$CONFIG_DIR/Secret_default_repaired.yaml"
    local status_file="${spec_file}.status"

    # `data` must be a string map; a scalar fails deserialization.
    cat > "$spec_file" << 'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: repaired
  namespace: default
type: Opaque
data: 42
EOF

    local waited=0
    while [ $waited -lt 10 ]; do
        if [ -f "$status_file" ] && grep -q "ParseError" "$status_file"; then
            break
        fi
        sleep 1
        waited=$((waited + 1))
    done
    if [ ! -f "$status_file" ] || ! grep -q "ParseError" "$status_file"; then
        log_error "ParseError sidecar did not appear at $status_file"
        [ -f "$status_file" ] && cat "$status_file"
        return 1
    fi
    log_success "ParseError sidecar written for the broken Secret"

    cat > "$spec_file" << 'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: repaired
  namespace: default
type: Opaque
data:
  token: aGVsbG8=
EOF

    waited=0
    while [ $waited -lt 10 ]; do
        if [ ! -f "$status_file" ] || ! grep -q "ParseError" "$status_file"; then
            log_success "Sidecar no longer reports ParseError after repair"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    log_error "Repaired Secret still reports its previous failure"
    cat "$status_file"
    return 1
}

# Test 11: Same (kind, key) in two subdirs — diagnostics endpoint surfaces conflict
test_collision_diagnostics() {
    log_section "Test 11: Cross-subdir collision exposed via /diagnostics/conf-conflicts"

    mkdir -p "$CONFIG_DIR/a" "$CONFIG_DIR/b"
    cat > "$CONFIG_DIR/a/HTTPRoute_default_dup.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: dup
  namespace: default
spec:
  parentRefs:
    - name: test-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /a
EOF
    cat > "$CONFIG_DIR/b/HTTPRoute_default_dup.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: dup
  namespace: default
spec:
  parentRefs:
    - name: test-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /b
EOF

    log_info "Wrote a/HTTPRoute_default_dup.yaml and b/HTTPRoute_default_dup.yaml; waiting ${WAIT_SECS}s..."
    sleep $WAIT_SECS

    local response
    response=$(curl_admin "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/diagnostics/conf-conflicts")
    if echo "$response" | grep -q "\"key\":\"default/dup\""; then
        log_success "Conflict for HTTPRoute default/dup surfaced via diagnostics endpoint"
    else
        log_error "Diagnostics did not list the expected conflict"
        echo "Response: $response"
        return 1
    fi

    if echo "$response" | grep -q "/a/HTTPRoute_default_dup.yaml"; then
        log_success "Winner path is from a/ (lex-smallest)"
    else
        log_error "Expected winner from a/ subdirectory"
        echo "Response: $response"
        return 1
    fi
}

# Test 12: API POST creates new resource at <conf>/<ns>/Kind_ns_name.yaml
test_api_create_namespace_subdir() {
    log_section "Test 12: API POST lands new spec at namespace subdir"

    local body='{"apiVersion":"v1","kind":"Service","metadata":{"name":"api-svc","namespace":"default"},"spec":{"ports":[{"port":8081,"targetPort":8081}],"selector":{"app":"backend"}}}'
    local response
    response=$(curl_admin -X POST -H "Content-Type: application/json" \
        -d "$body" \
        "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/service/default")
    log_info "POST response: $response"

    sleep 2  # allow PollWatcher to observe + storage to write

    local expected="$CONFIG_DIR/default/Service_default_api-svc.yaml"
    if [ -f "$expected" ]; then
        log_success "API-created Service spec landed at $expected"
    else
        log_error "Expected $expected to exist"
        log_info "Files matching api-svc:"
        find "$CONFIG_DIR" -name 'Service_default_api-svc*' || true
        return 1
    fi
}

# Test 13: POST against an existing legacy-flat resource returns 409 (no cross-subdir clone)
# Guards storage::create_one's registry-aware AlreadyExists path. Without it, POST would
# silently write a second copy at <conf>/<ns>/Kind_ns_name.yaml next to the legacy flat
# file, surfacing as a /diagnostics/conf-conflicts entry instead of a clean 409.
test_api_create_returns_conflict_for_legacy_flat() {
    log_section "Test 13: POST returns 409 when a legacy flat-layout resource already exists"

    # Pre-place an HTTPRoute at the legacy flat path (operator from pre-Path-A deployment).
    cat > "$CONFIG_DIR/HTTPRoute_default_legacyx.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: legacyx
  namespace: default
spec:
  parentRefs:
    - name: test-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /legacy
EOF
    log_info "Pre-placed flat <conf>/HTTPRoute_default_legacyx.yaml; waiting ${WAIT_SECS}s for watcher to record..."
    sleep $WAIT_SECS

    # POST a new HTTPRoute with the same (kind, ns, name). storage::create_one must
    # see the registry winner and reject with AlreadyExists → HTTP 409.
    ensure_admin_token
    local body='{"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"name":"legacyx","namespace":"default"},"spec":{"parentRefs":[{"name":"test-gateway"}],"rules":[{"matches":[{"path":{"type":"PathPrefix","value":"/new"}}]}]}}'
    local code
    code=$(curl -s -o /tmp/edgion_fw_post_body -w '%{http_code}' \
        -H "Authorization: Bearer $ADMIN_TOKEN" \
        -X POST -H "Content-Type: application/json" \
        -d "$body" \
        "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/httproute/default")
    log_info "POST status=$code; body=$(cat /tmp/edgion_fw_post_body 2>/dev/null)"

    if [ "$code" != "409" ]; then
        log_error "Expected HTTP 409 Conflict, got $code"
        return 1
    fi
    log_success "POST returned 409 Conflict as expected"

    # Belt and suspenders: no nested duplicate should have been written.
    local nested_dup="$CONFIG_DIR/default/HTTPRoute_default_legacyx.yaml"
    if [ -f "$nested_dup" ]; then
        log_error "Unexpected duplicate created at $nested_dup (storage::create_one bypassed registry)"
        return 1
    fi
    log_success "No nested duplicate created — legacy flat layout preserved"

    # And the legacy file itself is untouched.
    if ! grep -q "/legacy" "$CONFIG_DIR/HTTPRoute_default_legacyx.yaml"; then
        log_error "Legacy flat file's content changed unexpectedly"
        return 1
    fi
    log_success "Legacy flat file content preserved"
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
    test_delete_file                 || failed=1
    test_nested_dir_load                       || failed=1
    test_nested_status_follows_spec            || failed=1
    test_error_status_cleared_after_repair     || failed=1
    test_collision_diagnostics                 || failed=1
    test_api_create_namespace_subdir           || failed=1
    test_api_create_returns_conflict_for_legacy_flat || failed=1

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
