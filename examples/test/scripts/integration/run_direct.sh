#!/bin/bash
# =============================================================================
# Edgion Testscript
# Test test_client_direct  test_server （passed Gateway）
# =============================================================================

set -e

# 
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# projectdirectory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# Workdirectory
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/direct_${TIMESTAMP}"

# directory
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"

# test_server config
TEST_SERVER_HTTP_PORT=30001
TEST_SERVER_GRPC_PORT=30021
TEST_SERVER_WS_PORT=30005
TEST_SERVER_TCP_PORT=30010
TEST_SERVER_UDP_PORT=30011

# =============================================================================
# log
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
# healthCheck
# =============================================================================

# Waitport
wait_for_port() {
    local port=$1
    local service_name=$2
    local pid_file=$3
    local timeout=${4:-30}
    local elapsed=0
    
    log_info "Wait $service_name (port $port)..."
    while [ $elapsed -lt $timeout ]; do
        # Checkprocess
        if [ -f "$pid_file" ]; then
            if ! kill -0 $(cat "$pid_file") 2>/dev/null; then
                log_error "$service_name process"
                return 1
            fi
        fi
        
        # Checkport
        if nc -z 127.0.0.1 $port 2>/dev/null; then
            log_success "$service_name ready (port $port)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name  ${timeout}s Start"
    return 1
}

# Wait HTTP 
wait_for_http() {
    local url=$1
    local service_name=$2
    local timeout=${3:-30}
    local elapsed=0
    
    log_info "Wait $service_name (HTTP: $url)..."
    while [ $elapsed -lt $timeout ]; do
        if curl -sf -o /dev/null "$url" 2>/dev/null; then
            log_success "$service_name ready (HTTP )"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_warn "$service_name HTTP ，..."
    return 0
}

# Cleanup
cleanup() {
    echo ""
    log_info "Stopallservice..."
    
    if [ -f "${PID_DIR}/test_server.pid" ]; then
        local pid=$(cat "${PID_DIR}/test_server.pid")
        if kill -0 $pid 2>/dev/null; then
            kill $pid 2>/dev/null || true
            log_info "alreadyStop test_server (PID: $pid)"
        fi
        rm -f "${PID_DIR}/test_server.pid"
    fi
    
    log_success "Cleanupcompleted"
    echo ""
    log_info "Workdirectory: ${WORK_DIR}"
    log_info "logfile: ${LOG_DIR}/test_server.log"
}

# capture
trap cleanup EXIT SIGINT SIGTERM

# =============================================================================
# Start test_server
# =============================================================================
start_test_server() {
    log_section "Start test_server"
    
    # Cleanupmayprocess
    pkill -f "test_server" 2>/dev/null && log_info "Stop test_server" || true
    sleep 1
    
    # CheckBuild
    local test_server_bin="${PROJECT_ROOT}/target/debug/examples/test_server"
    if [ ! -f "$test_server_bin" ]; then
        log_error "test_server Build，PleaseRun prepare.sh"
        exit 1
    fi
    
    log_info "Start test_server..."
    "$test_server_bin" \
        --http-ports "30001,30002,30003" \
        --grpc-ports "30021,30022,30023" \
        --websocket-port $TEST_SERVER_WS_PORT \
        --tcp-port $TEST_SERVER_TCP_PORT \
        --udp-port $TEST_SERVER_UDP_PORT \
        --log-level info \
        > "${LOG_DIR}/test_server.log" 2>&1 &
    echo $! > "${PID_DIR}/test_server.pid"
    
    # Wait HTTP port
    if ! wait_for_port $TEST_SERVER_HTTP_PORT "test_server HTTP" "${PID_DIR}/test_server.pid" 30; then
        log_error "test_server Startfailed"
        log_info "viewlog: ${LOG_DIR}/test_server.log"
        tail -20 "${LOG_DIR}/test_server.log" 2>/dev/null || true
        exit 1
    fi
    
    # verify health 
    wait_for_http "http://127.0.0.1:${TEST_SERVER_HTTP_PORT}/health" "test_server" 10
    
    log_success "test_server Startsuccess (PID: $(cat ${PID_DIR}/test_server.pid))"
}

# =============================================================================
# RunTest
# =============================================================================
run_tests() {
    local test_command="${1:-all}"
    
    log_section "RunTest: $test_command"
    
    # Check test_client_direct
    local test_client_bin="${PROJECT_ROOT}/target/debug/examples/test_client_direct"
    if [ ! -f "$test_client_bin" ]; then
        log_error "test_client_direct Build，PleaseRun prepare.sh"
        exit 1
    fi
    
    log_info "RunTest..."
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
# 
# =============================================================================
main() {
    local test_command="${1:-all}"
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion Test${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Test: test_client_direct <-> test_server"
    echo -e ": $test_command"
    echo -e "Workdirectory: ${WORK_DIR}"
    echo ""
    
    cd "$PROJECT_ROOT"
    
    # Workdirectory
    mkdir -p "$LOG_DIR"
    mkdir -p "$PID_DIR"
    
    # Start test_server
    start_test_server
    
    # RunTest
    local test_result=0
    if run_tests "$test_command"; then
        log_section "Test"
        log_success "allTestpassed!"
    else
        log_section "Test"
        log_error "partialTestfailed!"
        test_result=1
    fi
    
    exit $test_result
}

main "$@"
