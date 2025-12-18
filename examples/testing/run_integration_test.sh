#!/bin/bash
# Edgion Integration Test Script
# 自动启动所有服务并运行集成测试

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 脚本所在目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# 项目根目录
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

# 日志目录
LOG_DIR="${SCRIPT_DIR}/logs"
mkdir -p "$LOG_DIR"

# 运行时目录
RUNTIME_DIR="${SCRIPT_DIR}/runtime"
mkdir -p "$RUNTIME_DIR"

# 日志文件
CONTROLLER_LOG="${LOG_DIR}/controller.log"
GATEWAY_LOG="${LOG_DIR}/gateway.log"
TEST_SERVER_LOG="${LOG_DIR}/test_server.log"
ACCESS_LOG="${LOG_DIR}/access.log"
TEST_RESULT_LOG="${LOG_DIR}/test_result.log"

# PID 文件
PID_DIR="${LOG_DIR}/pids"
mkdir -p "$PID_DIR"

echo_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

echo_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

echo_warn() {
    echo -e "${YELLOW}[!]${NC} $1"
}

echo_error() {
    echo -e "${RED}[✗]${NC} $1"
}

# 清理函数
cleanup() {
    echo ""
    echo_info "Stopping all services..."
    
    if [ -f "${PID_DIR}/gateway.pid" ]; then
        kill $(cat "${PID_DIR}/gateway.pid") 2>/dev/null || true
        rm -f "${PID_DIR}/gateway.pid"
    fi
    
    if [ -f "${PID_DIR}/controller.pid" ]; then
        kill $(cat "${PID_DIR}/controller.pid") 2>/dev/null || true
        rm -f "${PID_DIR}/controller.pid"
    fi
    
    if [ -f "${PID_DIR}/test_server.pid" ]; then
        kill $(cat "${PID_DIR}/test_server.pid") 2>/dev/null || true
        rm -f "${PID_DIR}/test_server.pid"
    fi
    
    echo_success "All services stopped"
}

# 捕获 Ctrl+C 和错误
trap cleanup EXIT SIGINT SIGTERM

echo ""
echo "=========================================="
echo "  Edgion Integration Test"
echo "=========================================="
echo ""

# 清理旧进程
echo_info "Cleaning up old processes..."
pkill -f edgion-controller 2>/dev/null && echo "         Stopped old controller" || true
pkill -f edgion-gateway 2>/dev/null && echo "         Stopped old gateway" || true
pkill -f "test_server" 2>/dev/null && echo "         Stopped old test_server" || true
sleep 1

# 清空旧日志
> "$CONTROLLER_LOG"
> "$GATEWAY_LOG"
> "$TEST_SERVER_LOG"
> "$ACCESS_LOG"
> "$TEST_RESULT_LOG"

# 1. 启动 test_server
echo_info "Starting test_server..."
cd "$PROJECT_DIR"
cargo run --example test_server > "$TEST_SERVER_LOG" 2>&1 &
echo $! > "${PID_DIR}/test_server.pid"
sleep 1

if kill -0 $(cat "${PID_DIR}/test_server.pid") 2>/dev/null; then
    echo_success "test_server started (PID: $(cat ${PID_DIR}/test_server.pid), ports: 30001-30004)"
else
    echo_error "Failed to start test_server"
    echo "         Log: $TEST_SERVER_LOG"
    echo "         Manual: cd $PROJECT_DIR && cargo run --example test_server"
    exit 1
fi

# 2. 启动 edgion-controller
echo_info "Starting edgion-controller (using default config)..."
cargo run --bin edgion-controller > "$CONTROLLER_LOG" 2>&1 &
echo $! > "${PID_DIR}/controller.pid"
sleep 1

if kill -0 $(cat "${PID_DIR}/controller.pid") 2>/dev/null; then
    echo_success "edgion-controller started (PID: $(cat ${PID_DIR}/controller.pid))"
else
    echo_error "Failed to start controller"
    echo "         Log: $CONTROLLER_LOG"
    echo "         Manual: cd $PROJECT_DIR && cargo run --bin edgion-controller"
    exit 1
fi

# 3. 启动 edgion-gateway
echo_info "Starting edgion-gateway (using default config)..."
EDGION_ACCESS_LOG="$ACCESS_LOG" \
cargo run --bin edgion-gateway > "$GATEWAY_LOG" 2>&1 &
echo $! > "${PID_DIR}/gateway.pid"
sleep 1

if kill -0 $(cat "${PID_DIR}/gateway.pid") 2>/dev/null; then
    echo_success "edgion-gateway started (PID: $(cat ${PID_DIR}/gateway.pid), port: 10080)"
else
    echo_error "Failed to start gateway"
    echo "         Log: $GATEWAY_LOG"
    echo "         Manual: cd $PROJECT_DIR && EDGION_ACCESS_LOG=$ACCESS_LOG cargo run --bin edgion-gateway"
    exit 1
fi

# 4. 等待服务完全启动
echo ""
echo_info "Waiting for services to be ready..."
echo "         Sleeping 10 seconds..."
sleep 10
echo_success "Services are ready"

# 5. 运行测试
echo ""
echo "=========================================="
echo "  Running Tests"
echo "=========================================="
echo ""

# Direct 模式测试
echo_info "Test 1: Direct mode (backend:30001)"
cargo run --example test_client -- http 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式测试
echo_info "Test 2: Gateway mode (gateway:10080)"
cargo run --example test_client -- -g http 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_RESULT=$?

# 6. 显示结果
echo ""
echo "=========================================="
echo "  Test Results"
echo "=========================================="
echo ""

if [ $DIRECT_RESULT -eq 0 ]; then
    echo_success "Direct mode test: PASSED"
else
    echo_error "Direct mode test: FAILED"
fi

if [ $GATEWAY_RESULT -eq 0 ]; then
    echo_success "Gateway mode test: PASSED"
else
    echo_error "Gateway mode test: FAILED"
fi

echo ""
echo "=========================================="
echo "  Logs"
echo "=========================================="
echo ""
echo "Controller:  $CONTROLLER_LOG"
echo "Gateway:     $GATEWAY_LOG"
echo "Test Server: $TEST_SERVER_LOG"
echo "Access Log:  $ACCESS_LOG"
echo "Test Result: $TEST_RESULT_LOG"
echo ""

# 显示 access.log 最后几行
if [ -f "$ACCESS_LOG" ] && [ -s "$ACCESS_LOG" ]; then
    echo ""
    echo "Last 10 lines of access.log:"
    echo "---"
    tail -n 10 "$ACCESS_LOG"
    echo ""
fi

# 返回测试结果
if [ $DIRECT_RESULT -eq 0 ] && [ $GATEWAY_RESULT -eq 0 ]; then
    echo_success "All tests PASSED! ✨"
    exit 0
else
    echo_error "Some tests FAILED!"
    exit 1
fi

