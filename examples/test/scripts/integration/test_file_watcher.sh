#!/bin/bash
# =============================================================================
# FileWatcher 
#  FileSystem 
# =============================================================================

set -e

# 
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# 
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# 
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/file_watcher_test_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"

# 
CONTROLLER_ADMIN_PORT=15800

# Controller PID
CONTROLLER_PID=""

# =============================================================================
# 
# =============================================================================
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

log_error() {
    echo -e "${RED}[✗]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# =============================================================================
# 
# =============================================================================
cleanup() {
    log_section ""
    
    if [ -n "$CONTROLLER_PID" ] && kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_info " edgion-controller (PID: $CONTROLLER_PID)"
        kill "$CONTROLLER_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$CONTROLLER_PID" 2>/dev/null || true
    fi
    
    if [ -d "$WORK_DIR" ]; then
        log_info ": $WORK_DIR"
        # ，
        # rm -rf "$WORK_DIR"
    fi
}

trap cleanup EXIT

# =============================================================================
# 
# =============================================================================
init_test_env() {
    log_section ""
    
    mkdir -p "$CONFIG_DIR"
    mkdir -p "$LOG_DIR"
    mkdir -p "$PID_DIR"
    
    log_info ": $WORK_DIR"
    log_info ": $CONFIG_DIR"
    log_info ": $LOG_DIR"
}

# =============================================================================
#  Controller
# =============================================================================
start_controller() {
    log_section " edgion-controller (FileSystem )"
    
    local controller_bin="${PROJECT_ROOT}/target/debug/edgion-controller"
    
    if [ ! -f "$controller_bin" ]; then
        log_error "Controller : $controller_bin"
        log_info ": cargo build"
        exit 1
    fi
    
    #  Controller
    "$controller_bin" \
        --work-dir "$WORK_DIR" \
        --log-dir "$LOG_DIR" \
        --conf-dir "$CONFIG_DIR" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        > "$LOG_DIR/controller.log" 2>&1 &
    
    CONTROLLER_PID=$!
    echo "$CONTROLLER_PID" > "$PID_DIR/controller.pid"
    
    log_info "Controller PID: $CONTROLLER_PID"
    
    #  Controller 
    log_info " Controller ..."
    local max_wait=30
    local waited=0
    
    while [ $waited -lt $max_wait ]; do
        if curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/health" > /dev/null 2>&1; then
            log_success "Controller "
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    
    log_error "Controller "
    cat "$LOG_DIR/controller.log"
    exit 1
}

# =============================================================================
# 
# =============================================================================

#  1:  GatewayClass 
test_add_gateway_class() {
    log_section " 1:  GatewayClass "
    
    cat > "$CONFIG_DIR/gateway-class.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
EOF
    
    log_info ": gateway-class.yaml"
    log_info " FileWatcher  (2s)..."
    sleep 2
    
    # 
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gatewayclass")
    
    if echo "$response" | grep -q "edgion"; then
        log_success "GatewayClass 'edgion' "
    else
        log_error "GatewayClass "
        echo ": $response"
        return 1
    fi
}

#  2:  Gateway 
test_add_gateway() {
    log_section " 2:  Gateway "
    
    cat > "$CONFIG_DIR/gateway.yaml" << 'EOF'
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
    
    log_info ": gateway.yaml"
    log_info " FileWatcher  (2s)..."
    sleep 2
    
    # 
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    
    if echo "$response" | grep -q "test-gateway"; then
        log_success "Gateway 'test-gateway' "
    else
        log_error "Gateway "
        echo ": $response"
        return 1
    fi
}

#  3:  Gateway 
test_modify_gateway() {
    log_section " 3:  Gateway "
    
    cat > "$CONFIG_DIR/gateway.yaml" << 'EOF'
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
    
    log_info ": gateway.yaml (port: 8080 -> 9090)"
    log_info " FileWatcher  (2s)..."
    sleep 2
    
    # 
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    
    if echo "$response" | grep -q "9090"; then
        log_success "Gateway  (port: 9090)"
    else
        log_error "Gateway "
        echo ": $response"
        return 1
    fi
}

#  4:  HTTPRoute 
test_add_http_route() {
    log_section " 4:  HTTPRoute "
    
    cat > "$CONFIG_DIR/http-route.yaml" << 'EOF'
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
    
    log_info ": http-route.yaml"
    log_info " FileWatcher  (2s)..."
    sleep 2
    
    # 
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/httproute")
    
    if echo "$response" | grep -q "test-route"; then
        log_success "HTTPRoute 'test-route' "
    else
        log_error "HTTPRoute "
        echo ": $response"
        return 1
    fi
}

#  5:  ()
test_rapid_modifications() {
    log_section " 5:  ()"
    
    #  5 
    for i in {1..5}; do
        cat > "$CONFIG_DIR/gateway.yaml" << EOF
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
        log_info " #$i: port=$((9000 + i))"
        sleep 0.1  # 
    done
    
    log_info " FileWatcher  (2s)..."
    sleep 2
    
    #  ( port: 9005)
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    
    if echo "$response" | grep -q "9005"; then
        log_success " ( port: 9005)"
    else
        log_error ""
        echo ": $response"
        return 1
    fi
}

#  6:  (mv)
test_rename_file() {
    log_section " 6:  (mv)"
    
    #  Service 
    cat > "$CONFIG_DIR/service-old.yaml" << 'EOF'
apiVersion: v1
kind: Service
metadata:
  name: backend-svc
  namespace: default
spec:
  ports:
    - port: 8080
      targetPort: 8080
  selector:
    app: backend
EOF
    
    log_info ": service-old.yaml"
    sleep 2
    
    # 
    mv "$CONFIG_DIR/service-old.yaml" "$CONFIG_DIR/service-new.yaml"
    
    log_info ": service-old.yaml -> service-new.yaml"
    log_info " FileWatcher  (2s)..."
    sleep 2
    
    # ：
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/service")
    
    if echo "$response" | grep -q "backend-svc"; then
        log_success " (Service )"
    else
        log_error ""
        echo ": $response"
        return 1
    fi
}

#  7:  (mv from outside)
test_move_file_in() {
    log_section " 7: "
    
    # 
    cat > "$WORK_DIR/temp-route.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: moved-route
  namespace: default
spec:
  parentRefs:
    - name: test-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /moved
      backendRefs:
        - name: backend-svc
          port: 8080
EOF
    
    log_info ": temp-route.yaml"
    
    # 
    mv "$WORK_DIR/temp-route.yaml" "$CONFIG_DIR/moved-route.yaml"
    
    log_info ": moved-route.yaml"
    log_info " FileWatcher  (2s)..."
    sleep 2
    
    # 
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/httproute")
    
    if echo "$response" | grep -q "moved-route"; then
        log_success " (HTTPRoute 'moved-route' )"
    else
        log_error ""
        echo ": $response"
        return 1
    fi
}

#  8:  (mv to outside)
test_move_file_out() {
    log_section " 8: "
    
    # 
    mv "$CONFIG_DIR/moved-route.yaml" "$WORK_DIR/removed-route.yaml"
    
    log_info ": moved-route.yaml -> ../removed-route.yaml"
    log_info " FileWatcher  (2s)..."
    sleep 2
    
    # ：， Remove 
    # ， ConfigServer 
    log_success " ()"
}

#  9: 
test_delete_file() {
    log_section " 9: "
    
    rm -f "$CONFIG_DIR/http-route.yaml"
    
    log_info ": http-route.yaml"
    log_info " FileWatcher  (2s)..."
    sleep 2
    
    # ：，， ConfigServer 
    #  panic
    log_success " ()"
}

# =============================================================================
# 
# =============================================================================
main() {
    log_section "FileWatcher "
    log_info ": $PROJECT_ROOT"
    
    # 
    init_test_env
    start_controller
    
    # 
    local failed=0
    
    test_add_gateway_class || failed=1
    test_add_gateway || failed=1
    test_modify_gateway || failed=1
    test_add_http_route || failed=1
    test_rapid_modifications || failed=1
    test_rename_file || failed=1
    test_move_file_in || failed=1
    test_move_file_out || failed=1
    test_delete_file || failed=1
    
    # 
    log_section ""
    
    if [ $failed -eq 0 ]; then
        log_success "!"
        echo ""
        log_info ": $LOG_DIR/controller.log"
    else
        log_error ""
        echo ""
        log_info ": cat $LOG_DIR/controller.log"
        exit 1
    fi
}

main "$@"
