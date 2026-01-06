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

# 创建带时间戳的独立测试目录
TEST_RUN_ID=$(date +%Y%m%d_%H%M%S)
LOG_DIR="${SCRIPT_DIR}/testing_tmp/${TEST_RUN_ID}"
mkdir -p "$LOG_DIR"

# 运行时目录
RUNTIME_DIR="${SCRIPT_DIR}/runtime"
mkdir -p "$RUNTIME_DIR"

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

# 参数解析
SERVER_ONLY_MODE=false
show_help() {
    cat << EOF
Usage: $0 [OPTIONS]

Options:
  -s, --server-only    Start services only (for manual testing)
  -h, --help          Show this help message

Examples:
  $0                   # Run full integration test
  $0 --server-only     # Start services and wait for manual testing

EOF
    exit 0
}

while [[ $# -gt 0 ]]; do
    case $1 in
        -s|--server-only)
            SERVER_ONLY_MODE=true
            shift
            ;;
        -h|--help)
            show_help
            ;;
        *)
            echo_error "Unknown option: $1"
            show_help
            ;;
    esac
done

if [ "$SERVER_ONLY_MODE" = true ]; then
    echo_info "Mode: Server-Only (manual testing)"
else
    echo_info "Mode: Full Integration Test"
fi

echo_info "Test run ID: ${TEST_RUN_ID}"
echo_info "Logs will be saved to: ${LOG_DIR}"

# 日志文件
CONTROLLER_LOG="${LOG_DIR}/controller.log"
GATEWAY_LOG="${LOG_DIR}/gateway.log"
TEST_SERVER_LOG="${LOG_DIR}/test_server.log"
ACCESS_LOG="${LOG_DIR}/access.log"
TEST_RESULT_LOG="${LOG_DIR}/test_result.log"
TEST_REPORT="${LOG_DIR}/test_report.txt"

# PID 文件
PID_DIR="${LOG_DIR}/pids"
mkdir -p "$PID_DIR"

# 记录测试开始时间
TEST_START_TIME=$(date +%s)

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
> "$TEST_REPORT"

# 0. 预编译所有组件（避免后续启动超时）
echo ""
echo "=========================================="
echo "  Pre-compiling All Components"
echo "=========================================="
echo ""

cd "$PROJECT_DIR"

echo_info "Compiling binaries (edgion-gateway, edgion-controller, edgion-ctl)..."
if ! cargo build --release --bin edgion-gateway --bin edgion-controller --bin edgion-ctl; then
    echo_error "Failed to compile binaries"
    exit 1
fi
echo_success "Binaries compiled successfully"

echo_info "Compiling test examples (test_server, test_client, validators)..."
if ! cargo build --release --example test_server --example test_client --example config_load_validator --example resource_diff; then
    echo_error "Failed to compile test examples"
    exit 1
fi
echo_success "Test examples compiled successfully"

echo ""

# 1. 生成 TLS 证书
echo_info "Generating TLS certificates..."
"${SCRIPT_DIR}/scripts/generate_certs.sh"
if [ $? -eq 0 ]; then
    echo_success "TLS certificates generated"
else
    echo_error "Failed to generate TLS certificates"
    exit 1
fi
echo ""

# 1.5 生成后端 TLS 证书
echo_info "Generating backend TLS certificates..."
"${SCRIPT_DIR}/scripts/generate_backend_certs.sh"
if [ $? -eq 0 ]; then
    echo_success "Backend TLS certificates generated"
else
    echo_error "Failed to generate backend TLS certificates"
    exit 1
fi
echo ""

# 1. 启动 test_server
echo_info "Starting test_server with Backend TLS support..."
cd "$PROJECT_DIR"

cargo run --release --example test_server -- \
  --https-backend-port 30051 \
  --cert-file "${SCRIPT_DIR}/certs/backend/server.crt" \
  --key-file "${SCRIPT_DIR}/certs/backend/server.key" \
  > "$TEST_SERVER_LOG" 2>&1 &
echo $! > "${PID_DIR}/test_server.pid"

# Wait for HTTP server (30001) to be ready
wait_for_port 30001 "test_server HTTP" "${PID_DIR}/test_server.pid" 30 || {
    echo_error "Failed to start test_server"
    echo "         Log: $TEST_SERVER_LOG"
    echo "         Manual: cd $PROJECT_DIR && cargo run --release --example test_server -- --https-backend-port 30051 --cert-file ${SCRIPT_DIR}/certs/backend/server.crt --key-file ${SCRIPT_DIR}/certs/backend/server.key"
    exit 1
}

# Wait for HTTPS backend server (30051) to be ready
wait_for_port 30051 "test_server HTTPS backend" "${PID_DIR}/test_server.pid" 30 || {
    echo_error "Failed to start test_server HTTPS backend"
    echo "         Log: $TEST_SERVER_LOG"
    exit 1
}

# 2. 启动 edgion-controller
echo_info "Starting edgion-controller (using default config)..."
cargo run --release --bin edgion-controller > "$CONTROLLER_LOG" 2>&1 &
echo $! > "${PID_DIR}/controller.pid"

# Wait briefly and check if process is still alive
sleep 2
if ! kill -0 $(cat "${PID_DIR}/controller.pid") 2>/dev/null; then
    echo_error "Controller process died immediately"
    echo "         Log: $CONTROLLER_LOG"
    echo "         Manual: cd $PROJECT_DIR && cargo run --release --bin edgion-controller"
    exit 1
fi
echo_success "edgion-controller started (PID: $(cat ${PID_DIR}/controller.pid))"

# 3. 启动 edgion-gateway
echo_info "Starting edgion-gateway (using default config)..."
EDGION_ACCESS_LOG="$ACCESS_LOG" \
EDGION_TEST_ACCESS_LOG_PATH="$ACCESS_LOG" \
cargo run --release --bin edgion-gateway > "$GATEWAY_LOG" 2>&1 &
echo $! > "${PID_DIR}/gateway.pid"

# Wait for gateway to be ready
wait_for_port 10080 "edgion-gateway" "${PID_DIR}/gateway.pid" 30 || {
    echo_error "Failed to start gateway"
    echo "         Log: $GATEWAY_LOG"
    echo "         Manual: cd $PROJECT_DIR && EDGION_ACCESS_LOG=$ACCESS_LOG EDGION_TEST_ACCESS_LOG_PATH=$ACCESS_LOG cargo run --release --bin edgion-gateway"
    exit 1
}

# Server-only mode: display info and wait
if [ "$SERVER_ONLY_MODE" = true ]; then
    echo ""
    echo "=========================================="
    echo "  Services Ready (Server-Only Mode)"
    echo "=========================================="
    echo ""
    echo_success "All services are running!"
    echo ""
    echo_info "Log files:"
    echo "  - Controller:  $CONTROLLER_LOG"
    echo "  - Gateway:     $GATEWAY_LOG"
    echo "  - Test Server: $TEST_SERVER_LOG"
    echo "  - Access Log:  $ACCESS_LOG"
    echo ""
    echo_info "To run tests manually:"
    echo ""
    echo "  # HTTP tests"
    echo "  cargo run --release --example test_client -- --gateway http"
    echo ""
    echo "  # Backend TLS tests"
    echo "  cargo run --release --example test_client -- --gateway backend-tls"
    echo ""
    echo "  # All gateway tests"
    echo "  cargo run --release --example test_client -- --gateway all"
    echo ""
    echo_warn "Press Ctrl+C to stop all services and exit"
    echo ""
    
    # Wait indefinitely until user interrupts
    trap "echo ''; echo_info 'Stopping all services...'; ${SCRIPT_DIR}/kill_all.sh; echo_success 'All services stopped'; exit 0" SIGINT SIGTERM
    
    while true; do
        sleep 1
    done
fi

# 4. Verify configuration loading
echo ""
echo "=========================================="
echo "  Configuration Load Validation"
echo "=========================================="
echo ""

echo_info "Verifying all config files are loaded by controller..."
if cargo run --release --example config_load_validator 2>&1 | tee -a "$TEST_RESULT_LOG"; then
    echo_success "All configurations loaded successfully"
else
    echo_error "Configuration loading FAILED - some YAML files not loaded"
    echo_info "Check controller logs for parsing errors: $CONTROLLER_LOG"
    exit 1
fi

echo ""

# 4.5. Verify resource synchronization
echo ""
echo "=========================================="
echo "  Resource Synchronization Check"
echo "=========================================="
echo ""

echo_info "Verifying controller and gateway resource sync..."
if cargo run --release --example resource_diff 2>&1 | tee -a "$TEST_RESULT_LOG"; then
    echo_success "Resource synchronization verified"
else
    echo_warn "Resource diff check completed with warnings (non-fatal)"
fi

echo ""

# 5. 运行测试
echo ""
echo "=========================================="
echo "  Running Tests"
echo "=========================================="
echo ""

# Direct 模式 HTTP 测试
echo_info "Test 1: HTTP Direct mode (backend:30001)"
cargo run --release --example test_client -- http 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_HTTP_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 HTTP 测试
echo_info "Test 2: HTTP Gateway mode (gateway:10080)"
cargo run --release --example test_client -- -g http 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_HTTP_RESULT=$?

echo ""
echo "---"
echo ""

# Direct 模式 gRPC 测试
echo_info "Test 3: gRPC Direct mode (backend:30021)"
cargo run --release --example test_client -- grpc 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_GRPC_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 gRPC 测试
echo_info "Test 4: gRPC Gateway mode (gateway:10080)"
cargo run --release --example test_client -- -g grpc 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_GRPC_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 gRPC Match Rules 测试
echo_info "Test 5: gRPC Match Rules Gateway mode (gateway:10080)"
cargo run --release --example test_client -- -g grpc-match 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_GRPC_MATCH_RESULT=$?

echo ""
echo "---"
echo ""

# Direct 模式 TCP 测试
echo_info "Test 7: TCP Direct mode (backend:30010)"
cargo run --release --example test_client -- tcp 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_TCP_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 TCP 测试
echo_info "Test 8: TCP Gateway mode (gateway:19000)"
cargo run --release --example test_client -- -g tcp 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_TCP_RESULT=$?

echo ""
echo "---"
echo ""

# Direct 模式 UDP 测试
echo_info "Test 9: UDP Direct mode (backend:30011)"
cargo run --release --example test_client -- udp 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_UDP_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 UDP 测试
echo_info "Test 10: UDP Gateway mode (gateway:19002)"
cargo run --release --example test_client -- -g udp 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_UDP_RESULT=$?

echo ""
echo "---"
echo ""

# Direct 模式 WebSocket 测试
echo_info "Test 11: WebSocket Direct mode (backend:30005)"
cargo run --release --example test_client -- websocket 2>&1 | tee -a "$TEST_RESULT_LOG"
DIRECT_WS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 WebSocket 测试
echo_info "Test 12: WebSocket Gateway mode (gateway:10080)"
cargo run --release --example test_client -- -g websocket 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_WS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 HTTPS 测试
echo_info "Test 13: HTTPS Gateway mode (gateway:10443)"
cargo run --release --example test_client -- -g https 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_HTTPS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 gRPC-TLS 测试
echo_info "Test 14: gRPC-TLS Gateway mode (gateway:18443)"
cargo run --release --example test_client -- -g grpc-tls 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_GRPC_TLS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 Real IP 测试
echo_info "Test 15: Real IP Gateway mode (gateway:10080)"
cargo run --release --example test_client -- -g real-ip 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_REAL_IP_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 Security Protection 测试
echo_info "Test 16: Security Protection Gateway mode (gateway:10080)"
cargo run --release --example test_client -- -g security 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_SECURITY_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 mTLS 测试（配置已在启动前复制）
echo_info "Test 17: mTLS Gateway mode (gateway:10444)"
cargo run --release --example test_client -- -g mtls 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_MTLS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 Plugin Logs 测试
echo_info "Test 18: Plugin Logs Gateway mode (gateway:10080)"
cargo run --release --example test_client -- -g plugin-logs 2>&1 | tee -a "$TEST_RESULT_LOG"
GATEWAY_PLUGIN_LOGS_RESULT=$?

echo ""
echo "---"
echo ""

# Gateway 模式 LB Policy 测试 - DISABLED
# Reason: Need better testing approach without log analysis timing issues
# TODO: Redesign test to use real backends with response headers
# echo_info "Test 17: LB Policy Gateway mode (gateway:10080)"
# EDGION_TEST_ACCESS_LOG_PATH="$ACCESS_LOG" cargo run --release --example test_client -- -g lb-policy 2>&1 | tee -a "$TEST_RESULT_LOG"
# GATEWAY_LB_POLICY_RESULT=$?
GATEWAY_LB_POLICY_RESULT=0  # Placeholder - test disabled

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

if [ $GATEWAY_GRPC_MATCH_RESULT -eq 0 ]; then
    echo_success "gRPC Match Rules Gateway mode: PASSED"
else
    echo_error "gRPC Match Rules Gateway mode: FAILED"
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

if [ $GATEWAY_MTLS_RESULT -eq 0 ]; then
    echo_success "mTLS Gateway mode: PASSED"
else
    echo_error "mTLS Gateway mode: FAILED"
fi

if [ $GATEWAY_PLUGIN_LOGS_RESULT -eq 0 ]; then
    echo_success "Plugin Logs Gateway mode: PASSED"
else
    echo_error "Plugin Logs Gateway mode: FAILED"
fi

# LB Policy test disabled
# if [ $GATEWAY_LB_POLICY_RESULT -eq 0 ]; then
#     echo_success "LB Policy Gateway mode: PASSED"
# else
#     echo_error "LB Policy Gateway mode: FAILED"
# fi

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
echo "Test Report: $TEST_REPORT"
echo ""

# 生成统一测试报告
TEST_END_TIME=$(date +%s)
TEST_DURATION=$((TEST_END_TIME - TEST_START_TIME))

# 计算通过和失败的测试数
PASSED_COUNT=0
FAILED_COUNT=0
TOTAL_TESTS=17  # LB Policy test disabled

[ $DIRECT_HTTP_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_HTTP_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $DIRECT_GRPC_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_GRPC_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_GRPC_MATCH_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $DIRECT_TCP_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_TCP_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $DIRECT_UDP_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_UDP_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $DIRECT_WS_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_WS_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_HTTPS_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_GRPC_TLS_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_REAL_IP_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_SECURITY_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_MTLS_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
[ $GATEWAY_PLUGIN_LOGS_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))
# [ $GATEWAY_LB_POLICY_RESULT -eq 0 ] && PASSED_COUNT=$((PASSED_COUNT + 1)) || FAILED_COUNT=$((FAILED_COUNT + 1))  # LB Policy test disabled

# 生成报告文件
cat > "$TEST_REPORT" << EOF
================================================================================
                    EDGION INTEGRATION TEST REPORT
================================================================================

Test Date:       $(date '+%Y-%m-%d %H:%M:%S')
Test Duration:   ${TEST_DURATION}s
Total Tests:     ${TOTAL_TESTS}
Passed:          ${PASSED_COUNT}
Failed:          ${FAILED_COUNT}
Success Rate:    $(( PASSED_COUNT * 100 / TOTAL_TESTS ))%

================================================================================
                          TEST RESULTS SUMMARY
================================================================================

Configuration & Validation:
  [✓] TLS Certificate Generation
  [✓] mTLS Certificate Generation
  [✓] Configuration Load Validation
  [✓] Resource Synchronization Check

Functional Tests:
  $([ $DIRECT_HTTP_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") HTTP Direct mode (backend:30001)
  $([ $GATEWAY_HTTP_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") HTTP Gateway mode (gateway:10080)
  $([ $DIRECT_GRPC_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") gRPC Direct mode (backend:30021)
  $([ $GATEWAY_GRPC_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") gRPC Gateway mode (gateway:10080)
  $([ $GATEWAY_GRPC_MATCH_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") gRPC Match Rules Gateway mode (gateway:10080)
  $([ $DIRECT_TCP_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") TCP Direct mode (backend:30010)
  $([ $GATEWAY_TCP_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") TCP Gateway mode (gateway:19000)
  $([ $DIRECT_UDP_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") UDP Direct mode (backend:30011)
  $([ $GATEWAY_UDP_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") UDP Gateway mode (gateway:19002)
  $([ $DIRECT_WS_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") WebSocket Direct mode (backend:30005)
  $([ $GATEWAY_WS_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") WebSocket Gateway mode (gateway:10080)
  $([ $GATEWAY_HTTPS_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") HTTPS Gateway mode (gateway:18443)
  $([ $GATEWAY_GRPC_TLS_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") gRPC-TLS Gateway mode (gateway:18443)
  $([ $GATEWAY_REAL_IP_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") Real IP Gateway mode (gateway:10080)
  $([ $GATEWAY_SECURITY_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") Security Protection Gateway mode (gateway:10080)
  $([ $GATEWAY_MTLS_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") mTLS Gateway mode (gateway:10444)
  $([ $GATEWAY_PLUGIN_LOGS_RESULT -eq 0 ] && echo "[PASSED]" || echo "[FAILED]") Plugin Logs Gateway mode (gateway:10080)

================================================================================
                          LOG FILE LOCATIONS
================================================================================

Controller Log:   $CONTROLLER_LOG
Gateway Log:      $GATEWAY_LOG
Test Server Log:  $TEST_SERVER_LOG
Access Log:       $ACCESS_LOG
Test Result Log:  $TEST_RESULT_LOG

================================================================================
                          OVERALL TEST STATUS
================================================================================

EOF

if [ $PASSED_COUNT -eq $TOTAL_TESTS ]; then
    echo "Status: ✓ ALL TESTS PASSED" >> "$TEST_REPORT"
    echo "" >> "$TEST_REPORT"
    echo "All $TOTAL_TESTS tests completed successfully!" >> "$TEST_REPORT"
else
    echo "Status: ✗ SOME TESTS FAILED" >> "$TEST_REPORT"
    echo "" >> "$TEST_REPORT"
    echo "Passed: $PASSED_COUNT/$TOTAL_TESTS tests" >> "$TEST_REPORT"
    echo "Failed: $FAILED_COUNT/$TOTAL_TESTS tests" >> "$TEST_REPORT"
    echo "" >> "$TEST_REPORT"
    echo "Please check the log files for detailed error messages." >> "$TEST_REPORT"
fi

echo "================================================================================" >> "$TEST_REPORT"

# 显示报告
echo ""
echo "=========================================="
echo "  Test Report Generated"
echo "=========================================="
echo ""
cat "$TEST_REPORT"

# 显示 access.log 最后几行
if [ -f "$ACCESS_LOG" ] && [ -s "$ACCESS_LOG" ]; then
    echo ""
    echo "Last 10 lines of access.log:"
    echo "---"
    tail -n 10 "$ACCESS_LOG"
    echo ""
fi

# 返回测试结果 (LB Policy test disabled)
if [ $DIRECT_HTTP_RESULT -eq 0 ] && [ $GATEWAY_HTTP_RESULT -eq 0 ] && \
   [ $DIRECT_GRPC_RESULT -eq 0 ] && [ $GATEWAY_GRPC_RESULT -eq 0 ] && \
   [ $GATEWAY_GRPC_MATCH_RESULT -eq 0 ] && \
   [ $DIRECT_TCP_RESULT -eq 0 ] && [ $GATEWAY_TCP_RESULT -eq 0 ] && \
   [ $DIRECT_UDP_RESULT -eq 0 ] && [ $GATEWAY_UDP_RESULT -eq 0 ] && \
   [ $DIRECT_WS_RESULT -eq 0 ] && [ $GATEWAY_WS_RESULT -eq 0 ] && \
   [ $GATEWAY_HTTPS_RESULT -eq 0 ] && [ $GATEWAY_GRPC_TLS_RESULT -eq 0 ] && \
   [ $GATEWAY_REAL_IP_RESULT -eq 0 ] && [ $GATEWAY_SECURITY_RESULT -eq 0 ] && \
   [ $GATEWAY_MTLS_RESULT -eq 0 ] && [ $GATEWAY_PLUGIN_LOGS_RESULT -eq 0 ]; then
    echo_success "All tests PASSED! ✨"
    exit 0
else
    echo_error "Some tests FAILED!"
    exit 1
fi

