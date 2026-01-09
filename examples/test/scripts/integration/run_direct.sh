#!/bin/bash
# =============================================================================
# Edgion 直接测试脚本
# 直接测试 test_client_direct 与 test_server 的连通性（不通过 Gateway）
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

# 创建时间戳工作目录
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/direct_${TIMESTAMP}"

# 子目录
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"

# test_server 配置
TEST_SERVER_HTTP_PORT=30001
TEST_SERVER_GRPC_PORT=30021
TEST_SERVER_WS_PORT=30005
TEST_SERVER_TCP_PORT=30010
TEST_SERVER_UDP_PORT=30011

# =============================================================================
# 日志函数
# =============================================================================
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
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
# 健康检查函数
# =============================================================================

# 等待端口可用
wait_for_port() {
    local port=$1
    local service_name=$2
    local pid_file=$3
    local timeout=${4:-30}
    local elapsed=0
    
    log_info "等待 $service_name (端口 $port)..."
    while [ $elapsed -lt $timeout ]; do
        # 检查进程是否存活
        if [ -f "$pid_file" ]; then
            if ! kill -0 $(cat "$pid_file") 2>/dev/null; then
                log_error "$service_name 进程意外退出"
                return 1
            fi
        fi
        
        # 检查端口是否开放
        if nc -z 127.0.0.1 $port 2>/dev/null; then
            log_success "$service_name 就绪 (端口 $port)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name 在 ${timeout}s 内未能启动"
    return 1
}

# 等待 HTTP 端点可用
wait_for_http() {
    local url=$1
    local service_name=$2
    local timeout=${3:-30}
    local elapsed=0
    
    log_info "等待 $service_name (HTTP: $url)..."
    while [ $elapsed -lt $timeout ]; do
        if curl -sf -o /dev/null "$url" 2>/dev/null; then
            log_success "$service_name 就绪 (HTTP 响应正常)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_warn "$service_name HTTP 端点未响应，继续..."
    return 0
}

# 清理函数
cleanup() {
    echo ""
    log_info "停止所有服务..."
    
    if [ -f "${PID_DIR}/test_server.pid" ]; then
        local pid=$(cat "${PID_DIR}/test_server.pid")
        if kill -0 $pid 2>/dev/null; then
            kill $pid 2>/dev/null || true
            log_info "已停止 test_server (PID: $pid)"
        fi
        rm -f "${PID_DIR}/test_server.pid"
    fi
    
    log_success "清理完成"
    echo ""
    log_info "工作目录: ${WORK_DIR}"
    log_info "日志文件: ${LOG_DIR}/test_server.log"
}

# 捕获退出信号
trap cleanup EXIT SIGINT SIGTERM

# =============================================================================
# 启动 test_server
# =============================================================================
start_test_server() {
    log_section "启动 test_server"
    
    # 先清理可能存在的旧进程
    pkill -f "test_server" 2>/dev/null && log_info "停止旧 test_server" || true
    sleep 1
    
    # 检查编译产物
    local test_server_bin="${PROJECT_ROOT}/target/debug/examples/test_server"
    if [ ! -f "$test_server_bin" ]; then
        log_error "test_server 未编译，请先运行 prepare.sh"
        exit 1
    fi
    
    log_info "启动 test_server..."
    "$test_server_bin" \
        --http-ports "30001,30002,30003" \
        --grpc-ports "30021,30022,30023" \
        --websocket-port $TEST_SERVER_WS_PORT \
        --tcp-port $TEST_SERVER_TCP_PORT \
        --udp-port $TEST_SERVER_UDP_PORT \
        --log-level info \
        > "${LOG_DIR}/test_server.log" 2>&1 &
    echo $! > "${PID_DIR}/test_server.pid"
    
    # 等待 HTTP 端口
    if ! wait_for_port $TEST_SERVER_HTTP_PORT "test_server HTTP" "${PID_DIR}/test_server.pid" 30; then
        log_error "test_server 启动失败"
        log_info "查看日志: ${LOG_DIR}/test_server.log"
        tail -20 "${LOG_DIR}/test_server.log" 2>/dev/null || true
        exit 1
    fi
    
    # 验证 health 端点
    wait_for_http "http://127.0.0.1:${TEST_SERVER_HTTP_PORT}/health" "test_server" 10
    
    log_success "test_server 启动成功 (PID: $(cat ${PID_DIR}/test_server.pid))"
}

# =============================================================================
# 运行测试
# =============================================================================
run_tests() {
    local test_command="${1:-all}"
    
    log_section "运行直接测试: $test_command"
    
    # 检查 test_client_direct
    local test_client_bin="${PROJECT_ROOT}/target/debug/examples/test_client_direct"
    if [ ! -f "$test_client_bin" ]; then
        log_error "test_client_direct 未编译，请先运行 prepare.sh"
        exit 1
    fi
    
    log_info "运行测试..."
    echo ""
    
    if "$test_client_bin" \
        --target-host 127.0.0.1 \
        --http-port $TEST_SERVER_HTTP_PORT \
        --grpc-port $TEST_SERVER_GRPC_PORT \
        --websocket-port $TEST_SERVER_WS_PORT \
        --tcp-port $TEST_SERVER_TCP_PORT \
        --udp-port $TEST_SERVER_UDP_PORT \
        "$test_command"; then
        return 0
    else
        return 1
    fi
}

# =============================================================================
# 主函数
# =============================================================================
main() {
    local test_command="${1:-all}"
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion 直接测试${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "测试: test_client_direct <-> test_server"
    echo -e "命令: $test_command"
    echo -e "工作目录: ${WORK_DIR}"
    echo ""
    
    cd "$PROJECT_ROOT"
    
    # 创建工作目录
    mkdir -p "$LOG_DIR"
    mkdir -p "$PID_DIR"
    
    # 启动 test_server
    start_test_server
    
    # 运行测试
    local test_result=0
    if run_tests "$test_command"; then
        log_section "测试结果"
        log_success "所有直接测试通过!"
    else
        log_section "测试结果"
        log_error "部分测试失败!"
        test_result=1
    fi
    
    exit $test_result
}

main "$@"
