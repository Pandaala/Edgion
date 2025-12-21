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

# Wait for a service to be ready by checking if it's listening on a port
# Usage: wait_for_port <port> <service_name> <pid_file> [timeout_seconds]
wait_for_port() {
    local port=$1
    local service_name=$2
    local pid_file=$3
    local timeout=${4:-30}  # default 30s timeout
    local elapsed=0
    
    echo_info "Waiting for $service_name (port $port)..."
    while [ $elapsed -lt $timeout ]; do
        # Check if process is still alive
        if [ -f "$pid_file" ]; then
            if ! kill -0 $(cat "$pid_file") 2>/dev/null; then
                echo_error "$service_name process died unexpectedly"
                return 1
            fi
        fi
        
        # Check if port is open
        if nc -z 127.0.0.1 $port 2>/dev/null; then
            echo_success "$service_name is ready (port $port)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    echo_error "$service_name failed to start within ${timeout}s"
    return 1
}

# Check if HTTP endpoint is responding
# Usage: wait_for_http <url> <service_name> [timeout_seconds]
wait_for_http() {
    local url=$1
    local service_name=$2
    local timeout=${3:-30}
    local elapsed=0
    
    echo_info "Waiting for $service_name (HTTP check: $url)..."
    while [ $elapsed -lt $timeout ]; do
        if curl -sf -o /dev/null "$url" 2>/dev/null; then
            echo_success "$service_name is ready (HTTP responding)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    echo_error "$service_name failed to start within ${timeout}s"
    return 1
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

# Check required tools
if ! command -v nc &> /dev/null; then
    echo_warn "netcat (nc) not found, will use alternative port checking"
fi

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

# 0. 生成 TLS 证书
echo_info "Generating TLS certificates..."
"${SCRIPT_DIR}/scripts/generate_certs.sh"
if [ $? -eq 0 ]; then
    echo_success "TLS certificates generated"
else
    echo_error "Failed to generate TLS certificates"
    exit 1
fi
echo ""

# 1. 启动 test_server
echo_info "Starting test_server..."
cd "$PROJECT_DIR"
cargo run --example test_server > "$TEST_SERVER_LOG" 2>&1 &
echo $! > "${PID_DIR}/test_server.pid"

# Wait for HTTP server (30001) to be ready
wait_for_port 30001 "test_server HTTP" "${PID_DIR}/test_server.pid" 30 || {
    echo_error "Failed to start test_server"
    echo "         Log: $TEST_SERVER_LOG"
    echo "         Manual: cd $PROJECT_DIR && cargo run --example test_server"
    exit 1
}

# 2. 启动 edgion-controller
echo_info "Starting edgion-controller (using default config)..."
cargo run --bin edgion-controller > "$CONTROLLER_LOG" 2>&1 &
echo $! > "${PID_DIR}/controller.pid"

# Wait briefly and check if process is still alive
sleep 2
if ! kill -0 $(cat "${PID_DIR}/controller.pid") 2>/dev/null; then
    echo_error "Controller process died immediately"
    echo "         Log: $CONTROLLER_LOG"
    echo "         Manual: cd $PROJECT_DIR && cargo run --bin edgion-controller"
    exit 1
fi
echo_success "edgion-controller started (PID: $(cat ${PID_DIR}/controller.pid))"

# 3. 启动 edgion-gateway
echo_info "Starting edgion-gateway (using default config)..."
EDGION_ACCESS_LOG="$ACCESS_LOG" \
cargo run --bin edgion-gateway > "$GATEWAY_LOG" 2>&1 &
echo $! > "${PID_DIR}/gateway.pid"

# Wait for gateway to be ready
wait_for_port 10080 "edgion-gateway" "${PID_DIR}/gateway.pid" 30 || {
    echo_error "Failed to start gateway"
    echo "         Log: $GATEWAY_LOG"
    echo "         Manual: cd $PROJECT_DIR && EDGION_ACCESS_LOG=$ACCESS_LOG cargo run --bin edgion-gateway"
    exit 1
}

# 5. 运行测试
echo ""
echo "=========================================="
echo "  Running Tests"
echo "=========================================="
echo ""

# Direct 模式 HTTP 测试
echo_info "Test 1: HTTP Direct mode (backend:30001)"
cargo run --example test_client -- http 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_HTTP_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 HTTP 测试
echo_info "Test 2: HTTP Gateway mode (gateway:10080)"
cargo run --example test_client -- -g http 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_HTTP_RESULT=$?

echo ""
echo "---"
echo ""

# Direct 模式 gRPC 测试
echo_info "Test 3: gRPC Direct mode (backend:30021)"
cargo run --example test_client -- grpc 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_GRPC_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 gRPC 测试
echo_info "Test 4: gRPC Gateway mode (gateway:10080)"
cargo run --example test_client -- -g grpc 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_GRPC_RESULT=$?

echo ""
echo "---"
echo ""

# Direct 模式 TCP 测试
echo_info "Test 5: TCP Direct mode (backend:30010)"
cargo run --example test_client -- tcp 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_TCP_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 TCP 测试
echo_info "Test 6: TCP Gateway mode (gateway:19000)"
cargo run --example test_client -- -g tcp 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_TCP_RESULT=$?

echo ""
echo "---"
echo ""

# Direct 模式 UDP 测试
echo_info "Test 7: UDP Direct mode (backend:30011)"
cargo run --example test_client -- udp 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_UDP_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 UDP 测试
echo_info "Test 8: UDP Gateway mode (gateway:19002)"
cargo run --example test_client -- -g udp 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_UDP_RESULT=$?

echo ""
echo "---"
echo ""

# Direct 模式 WebSocket 测试
echo_info "Test 9: WebSocket Direct mode (backend:30005)"
cargo run --example test_client -- websocket 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_WS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 WebSocket 测试
echo_info "Test 10: WebSocket Gateway mode (gateway:10080)"
cargo run --example test_client -- -g websocket 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_WS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 HTTPS 测试
echo_info "Test 11: HTTPS Gateway mode (gateway:18443)"
cargo run --example test_client -- -g https 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_HTTPS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 gRPC-TLS 测试
echo_info "Test 12: gRPC-TLS Gateway mode (gateway:18443)"
cargo run --example test_client -- -g grpc-tls 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_GRPC_TLS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 Real IP 测试
echo_info "Test 13: Real IP Gateway mode (gateway:10080)"
cargo run --example test_client -- -g real-ip 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_REAL_IP_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 Security 测试
echo_info "Test 14: Security Protection Gateway mode (gateway:10080)"
cargo run --example test_client -- -g security 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_SECURITY_RESULT=$?

# 6. 显示结果
echo ""
echo "=========================================="
echo "  Test Results"
echo "=========================================="
echo ""

if [ $DIRECT_HTTP_RESULT -eq 0 ]; then
    echo_success "HTTP Direct mode: PASSED"
else
    echo_error "HTTP Direct mode: FAILED"
fi

if [ $GATEWAY_HTTP_RESULT -eq 0 ]; then
    echo_success "HTTP Gateway mode: PASSED"
else
    echo_error "HTTP Gateway mode: FAILED"
fi

if [ $DIRECT_GRPC_RESULT -eq 0 ]; then
    echo_success "gRPC Direct mode: PASSED"
else
    echo_error "gRPC Direct mode: FAILED"
fi

if [ $GATEWAY_GRPC_RESULT -eq 0 ]; then
    echo_success "gRPC Gateway mode: PASSED"
else
    echo_error "gRPC Gateway mode: FAILED"
fi

if [ $DIRECT_TCP_RESULT -eq 0 ]; then
    echo_success "TCP Direct mode: PASSED"
else
    echo_error "TCP Direct mode: FAILED"
fi

if [ $GATEWAY_TCP_RESULT -eq 0 ]; then
    echo_success "TCP Gateway mode: PASSED"
else
    echo_error "TCP Gateway mode: FAILED"
fi

if [ $DIRECT_UDP_RESULT -eq 0 ]; then
    echo_success "UDP Direct mode: PASSED"
else
    echo_error "UDP Direct mode: FAILED"
fi

if [ $GATEWAY_UDP_RESULT -eq 0 ]; then
    echo_success "UDP Gateway mode: PASSED"
else
    echo_error "UDP Gateway mode: FAILED"
fi

if [ $DIRECT_WS_RESULT -eq 0 ]; then
    echo_success "WebSocket Direct mode: PASSED"
else
    echo_error "WebSocket Direct mode: FAILED"
fi

if [ $GATEWAY_WS_RESULT -eq 0 ]; then
    echo_success "WebSocket Gateway mode: PASSED"
else
    echo_error "WebSocket Gateway mode: FAILED"
fi

if [ $GATEWAY_HTTPS_RESULT -eq 0 ]; then
    echo_success "HTTPS Gateway mode: PASSED"
else
    echo_error "HTTPS Gateway mode: FAILED"
fi

if [ $GATEWAY_GRPC_TLS_RESULT -eq 0 ]; then
    echo_success "gRPC-TLS Gateway mode: PASSED"
else
    echo_error "gRPC-TLS Gateway mode: FAILED"
fi

if [ $GATEWAY_REAL_IP_RESULT -eq 0 ]; then
    echo_success "Real IP Gateway mode: PASSED"
else
    echo_error "Real IP Gateway mode: FAILED"
fi

if [ $GATEWAY_SECURITY_RESULT -eq 0 ]; then
    echo_success "Security Protection Gateway mode: PASSED"
else
    echo_error "Security Protection Gateway mode: FAILED"
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
if [ $DIRECT_HTTP_RESULT -eq 0 ] && [ $GATEWAY_HTTP_RESULT -eq 0 ] && \
   [ $DIRECT_GRPC_RESULT -eq 0 ] && [ $GATEWAY_GRPC_RESULT -eq 0 ] && \
   [ $DIRECT_TCP_RESULT -eq 0 ] && [ $GATEWAY_TCP_RESULT -eq 0 ] && \
   [ $DIRECT_UDP_RESULT -eq 0 ] && [ $GATEWAY_UDP_RESULT -eq 0 ] && \
   [ $DIRECT_WS_RESULT -eq 0 ] && [ $GATEWAY_WS_RESULT -eq 0 ] && \
   [ $GATEWAY_HTTPS_RESULT -eq 0 ] && [ $GATEWAY_GRPC_TLS_RESULT -eq 0 ] && \
   [ $GATEWAY_REAL_IP_RESULT -eq 0 ] && [ $GATEWAY_SECURITY_RESULT -eq 0 ]; then
    echo_success "All tests PASSED! ✨"
    exit 0
else
    echo_error "Some tests FAILED!"
    exit 1
fi

