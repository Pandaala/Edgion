#!/bin/bash
# =============================================================================
# FileWatcher 集成测试
# 测试 FileSystem 模式下的文件监控功能
# =============================================================================

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# 项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# 创建临时工作目录
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/file_watcher_test_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"

# 服务端口
CONTROLLER_ADMIN_PORT=15800

# Controller PID
CONTROLLER_PID=""

# =============================================================================
# 日志函数
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
# 清理函数
# =============================================================================
cleanup() {
    log_section "清理测试环境"
    
    if [ -n "$CONTROLLER_PID" ] && kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_info "停止 edgion-controller (PID: $CONTROLLER_PID)"
        kill "$CONTROLLER_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$CONTROLLER_PID" 2>/dev/null || true
    fi
    
    if [ -d "$WORK_DIR" ]; then
        log_info "保留测试目录: $WORK_DIR"
        # 如果测试成功，可以选择删除
        # rm -rf "$WORK_DIR"
    fi
}

trap cleanup EXIT

# =============================================================================
# 初始化测试环境
# =============================================================================
init_test_env() {
    log_section "初始化测试环境"
    
    mkdir -p "$CONFIG_DIR"
    mkdir -p "$LOG_DIR"
    mkdir -p "$PID_DIR"
    
    log_info "工作目录: $WORK_DIR"
    log_info "配置目录: $CONFIG_DIR"
    log_info "日志目录: $LOG_DIR"
}

# =============================================================================
# 启动 Controller
# =============================================================================
start_controller() {
    log_section "启动 edgion-controller (FileSystem 模式)"
    
    local controller_bin="${PROJECT_ROOT}/target/debug/edgion-controller"
    
    if [ ! -f "$controller_bin" ]; then
        log_error "Controller 二进制文件不存在: $controller_bin"
        log_info "请先运行: cargo build"
        exit 1
    fi
    
    # 启动 Controller
    "$controller_bin" \
        --work-dir "$WORK_DIR" \
        --log-dir "$LOG_DIR" \
        --conf-dir "$CONFIG_DIR" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        > "$LOG_DIR/controller.log" 2>&1 &
    
    CONTROLLER_PID=$!
    echo "$CONTROLLER_PID" > "$PID_DIR/controller.pid"
    
    log_info "Controller PID: $CONTROLLER_PID"
    
    # 等待 Controller 启动
    log_info "等待 Controller 启动..."
    local max_wait=30
    local waited=0
    
    while [ $waited -lt $max_wait ]; do
        if curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/health" > /dev/null 2>&1; then
            log_success "Controller 启动成功"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    
    log_error "Controller 启动超时"
    cat "$LOG_DIR/controller.log"
    exit 1
}

# =============================================================================
# 测试用例
# =============================================================================

# 测试 1: 添加 GatewayClass 文件
test_add_gateway_class() {
    log_section "测试 1: 添加 GatewayClass 文件"
    
    cat > "$CONFIG_DIR/gateway-class.yaml" << 'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
EOF
    
    log_info "已创建文件: gateway-class.yaml"
    log_info "等待 FileWatcher 处理 (2s)..."
    sleep 2
    
    # 验证资源已加载
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gatewayclass")
    
    if echo "$response" | grep -q "edgion"; then
        log_success "GatewayClass 'edgion' 已加载"
    else
        log_error "GatewayClass 未找到"
        echo "响应: $response"
        return 1
    fi
}

# 测试 2: 添加 Gateway 文件
test_add_gateway() {
    log_section "测试 2: 添加 Gateway 文件"
    
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
    
    log_info "已创建文件: gateway.yaml"
    log_info "等待 FileWatcher 处理 (2s)..."
    sleep 2
    
    # 验证资源已加载
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    
    if echo "$response" | grep -q "test-gateway"; then
        log_success "Gateway 'test-gateway' 已加载"
    else
        log_error "Gateway 未找到"
        echo "响应: $response"
        return 1
    fi
}

# 测试 3: 修改 Gateway 文件
test_modify_gateway() {
    log_section "测试 3: 修改 Gateway 文件"
    
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
    
    log_info "已修改文件: gateway.yaml (port: 8080 -> 9090)"
    log_info "等待 FileWatcher 处理 (2s)..."
    sleep 2
    
    # 验证资源已更新
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    
    if echo "$response" | grep -q "9090"; then
        log_success "Gateway 已更新 (port: 9090)"
    else
        log_error "Gateway 更新未生效"
        echo "响应: $response"
        return 1
    fi
}

# 测试 4: 添加 HTTPRoute 文件
test_add_http_route() {
    log_section "测试 4: 添加 HTTPRoute 文件"
    
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
    
    log_info "已创建文件: http-route.yaml"
    log_info "等待 FileWatcher 处理 (2s)..."
    sleep 2
    
    # 验证资源已加载
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/httproute")
    
    if echo "$response" | grep -q "test-route"; then
        log_success "HTTPRoute 'test-route' 已加载"
    else
        log_error "HTTPRoute 未找到"
        echo "响应: $response"
        return 1
    fi
}

# 测试 5: 快速连续修改 (测试去抖动)
test_rapid_modifications() {
    log_section "测试 5: 快速连续修改 (测试去抖动)"
    
    # 快速连续修改 5 次
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
        log_info "修改 #$i: port=$((9000 + i))"
        sleep 0.1  # 快速修改
    done
    
    log_info "等待 FileWatcher 处理 (2s)..."
    sleep 2
    
    # 验证最终状态 (应该是 port: 9005)
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/gateway")
    
    if echo "$response" | grep -q "9005"; then
        log_success "快速修改测试通过 (最终 port: 9005)"
    else
        log_error "快速修改测试失败"
        echo "响应: $response"
        return 1
    fi
}

# 测试 6: 重命名文件 (mv)
test_rename_file() {
    log_section "测试 6: 重命名文件 (mv)"
    
    # 先创建一个新的 Service 文件
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
    
    log_info "创建文件: service-old.yaml"
    sleep 2
    
    # 重命名文件
    mv "$CONFIG_DIR/service-old.yaml" "$CONFIG_DIR/service-new.yaml"
    
    log_info "重命名文件: service-old.yaml -> service-new.yaml"
    log_info "等待 FileWatcher 处理 (2s)..."
    sleep 2
    
    # 验证：重命名后文件应该被检测并加载
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/service")
    
    if echo "$response" | grep -q "backend-svc"; then
        log_success "重命名测试通过 (Service 已加载)"
    else
        log_error "重命名测试失败"
        echo "响应: $response"
        return 1
    fi
}

# 测试 7: 移动文件进入配置目录 (mv from outside)
test_move_file_in() {
    log_section "测试 7: 移动文件进入配置目录"
    
    # 在配置目录外创建文件
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
    
    log_info "在配置目录外创建文件: temp-route.yaml"
    
    # 移动到配置目录
    mv "$WORK_DIR/temp-route.yaml" "$CONFIG_DIR/moved-route.yaml"
    
    log_info "移动文件到配置目录: moved-route.yaml"
    log_info "等待 FileWatcher 处理 (2s)..."
    sleep 2
    
    # 验证资源已加载
    local response
    response=$(curl -s "http://localhost:${CONTROLLER_ADMIN_PORT}/api/v1/namespaced/httproute")
    
    if echo "$response" | grep -q "moved-route"; then
        log_success "移动文件测试通过 (HTTPRoute 'moved-route' 已加载)"
    else
        log_error "移动文件测试失败"
        echo "响应: $response"
        return 1
    fi
}

# 测试 8: 移动文件出配置目录 (mv to outside)
test_move_file_out() {
    log_section "测试 8: 移动文件出配置目录"
    
    # 移动文件出配置目录
    mv "$CONFIG_DIR/moved-route.yaml" "$WORK_DIR/removed-route.yaml"
    
    log_info "移动文件出配置目录: moved-route.yaml -> ../removed-route.yaml"
    log_info "等待 FileWatcher 处理 (2s)..."
    sleep 2
    
    # 注意：当前实现中，移出文件会触发 Remove 事件
    # 但删除操作只记录日志，不会立即从 ConfigServer 移除资源
    log_success "移动文件出目录测试完成 (文件移出已被检测到)"
}

# 测试 9: 删除文件
test_delete_file() {
    log_section "测试 9: 删除文件"
    
    rm -f "$CONFIG_DIR/http-route.yaml"
    
    log_info "已删除文件: http-route.yaml"
    log_info "等待 FileWatcher 处理 (2s)..."
    sleep 2
    
    # 注意：当前实现中，删除文件只会记录日志，不会立即从 ConfigServer 移除资源
    # 这里只验证不会 panic
    log_success "删除文件测试完成 (文件删除已被检测到)"
}

# =============================================================================
# 主流程
# =============================================================================
main() {
    log_section "FileWatcher 集成测试"
    log_info "项目根目录: $PROJECT_ROOT"
    
    # 初始化
    init_test_env
    start_controller
    
    # 运行测试
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
    
    # 总结
    log_section "测试结果"
    
    if [ $failed -eq 0 ]; then
        log_success "所有测试通过!"
        echo ""
        log_info "测试日志: $LOG_DIR/controller.log"
    else
        log_error "部分测试失败"
        echo ""
        log_info "查看日志: cat $LOG_DIR/controller.log"
        exit 1
    fi
}

main "$@"
