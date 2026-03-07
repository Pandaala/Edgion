#!/bin/bash
# =============================================================================
# Startall Edgion Testservice
# Start: test_server -> controller -> gateway
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
WORK_DIR="${PROJECT_ROOT}/integration_testing/testing_${TIMESTAMP}"

#  WORK_DIR scriptuse
export EDGION_WORK_DIR="$WORK_DIR"

# directory
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"

# configfile
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.toml"
GATEWAY_CONFIG="${PROJECT_ROOT}/config/edgion-gateway.toml"

# serviceport
TEST_SERVER_HTTP_PORT=30001
CONTROLLER_ADMIN_PORT=5800
GATEWAY_HTTP_PORT=10080

# =============================================================================
# log
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
# Cleanupallprocess ()
# =============================================================================
kill_all_processes() {
    log_section "Cleanupallprocess"
    
    #  kill allprocess
    pkill -9 -f "edgion-controller" 2>/dev/null && log_info "alreadyStop edgion-controller" || true
    pkill -9 -f "edgion-gateway" 2>/dev/null && log_info "alreadyStop edgion-gateway" || true
    pkill -9 -f "test_server" 2>/dev/null && log_info "alreadyStop test_server" || true
    
    # portrelease
    sleep 2
    
    # verifyportalreadyrelease
    local ports_busy=false
    if nc -z 127.0.0.1 $TEST_SERVER_HTTP_PORT 2>/dev/null; then
        log_error "port $TEST_SERVER_HTTP_PORT occupied"
        ports_busy=true
    fi
    if nc -z 127.0.0.1 $CONTROLLER_ADMIN_PORT 2>/dev/null; then
        log_error "port $CONTROLLER_ADMIN_PORT occupied"
        ports_busy=true
    fi
    if nc -z 127.0.0.1 $GATEWAY_HTTP_PORT 2>/dev/null; then
        log_error "port $GATEWAY_HTTP_PORT occupied"
        ports_busy=true
    fi
    
    if $ports_busy; then
        log_error "releaseport，PleaseCheck"
        exit 1
    fi
    
    log_success "allprocessalreadyCleanup，portalreadyrelease"
}

# =============================================================================
# Checkbinaryfile
# =============================================================================
check_binaries() {
    log_section "Checkbinaryfile"
    
    local missing=false
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/edgion-controller" ]; then
        log_error "edgion-controller Build"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/edgion-gateway" ]; then
        log_error "edgion-gateway Build"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/examples/test_server" ]; then
        log_error "test_server Build"
        missing=true
    fi
    
    if $missing; then
        log_error "PleaseRun prepare.sh Build"
        exit 1
    fi
    
    log_success "allbinaryfileready"
}

# =============================================================================
# Waitport
# =============================================================================
wait_for_port() {
    local port=$1
    local service_name=$2
    local pid=$3
    local timeout=${4:-30}
    local elapsed=0
    
    log_info "Wait $service_name (port $port)..."
    
    while [ $elapsed -lt $timeout ]; do
        # Checkprocess
        if ! kill -0 $pid 2>/dev/null; then
            log_error "$service_name processalready (PID: $pid)"
            return 1
        fi
        
        # Checkport
        if nc -z 127.0.0.1 $port 2>/dev/null; then
            log_success "$service_name portready (port $port)"
            return 0
        fi
        
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name  ${timeout}s Start"
    return 1
}

# =============================================================================
# Wait HTTP healthCheck
# =============================================================================
wait_for_health() {
    local url=$1
    local service_name=$2
    local timeout=${3:-10}
    local elapsed=0
    
    log_info "Check $service_name healthstatus..."
    
    while [ $elapsed -lt $timeout ]; do
        local response=$(curl -sf "$url" 2>/dev/null)
        if [ -n "$response" ]; then
            log_success "$service_name healthCheckpassed"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name healthCheckfailed"
    return 1
}

# =============================================================================
# Start test_server
# =============================================================================
start_test_server() {
    log_section "Start test_server"
    
    "${PROJECT_ROOT}/target/debug/examples/test_server" \
        --http-ports "30001,30002,30003" \
        --grpc-ports "30021,30022,30023" \
        --websocket-port 30005 \
        --tcp-port 30010 \
        --udp-port 30011 \
        --auth-port 30040 \
        --log-level info \
        > "${LOG_DIR}/test_server.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/test_server.pid"
    
    # Waitport
    if ! wait_for_port $TEST_SERVER_HTTP_PORT "test_server" $pid 30; then
        log_error "test_server Startfailed，viewlog: ${LOG_DIR}/test_server.log"
        tail -20 "${LOG_DIR}/test_server.log" 2>/dev/null || true
        exit 1
    fi
    
    # healthCheck
    if ! wait_for_health "http://127.0.0.1:${TEST_SERVER_HTTP_PORT}/health" "test_server" 10; then
        log_error "test_server healthCheckfailed"
        exit 1
    fi
    
    log_success "test_server Startsuccess (PID: $pid)"
}

# =============================================================================
# Start controller
# =============================================================================
start_controller() {
    log_section "Start edgion-controller"
    
    "${PROJECT_ROOT}/target/debug/edgion-controller" \
        -c "$CONTROLLER_CONFIG" \
        --work-dir "$PROJECT_ROOT" \
        --conf-dir "$CONFIG_DIR" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        --test-mode \
        > "${LOG_DIR}/controller.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/controller.pid"
    
    # Waitport
    if ! wait_for_port $CONTROLLER_ADMIN_PORT "edgion-controller" $pid 30; then
        log_error "edgion-controller Startfailed，viewlog: ${LOG_DIR}/controller.log"
        tail -20 "${LOG_DIR}/controller.log" 2>/dev/null || true
        exit 1
    fi
    
    # healthCheck
    if ! wait_for_health "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/health" "edgion-controller" 10; then
        log_error "edgion-controller healthCheckfailed"
        exit 1
    fi
    
    log_success "edgion-controller Startsuccess (PID: $pid)"
}

# =============================================================================
# Start gateway
# =============================================================================
start_gateway() {
    log_section "Start edgion-gateway"
    
    EDGION_ACCESS_LOG="${LOG_DIR}/access.log" \
    EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log" \
    "${PROJECT_ROOT}/target/debug/edgion-gateway" \
        -c "$GATEWAY_CONFIG" \
        --integration-testing-mode \
        > "${LOG_DIR}/gateway.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/gateway.pid"
    
    # Waitport
    if ! wait_for_port $GATEWAY_HTTP_PORT "edgion-gateway" $pid 30; then
        log_error "edgion-gateway Startfailed，viewlog: ${LOG_DIR}/gateway.log"
        tail -20 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi
    
    log_success "edgion-gateway Startsuccess (PID: $pid)"
}

# =============================================================================
# Prepareconfigfile
# =============================================================================
prepare_config() {
    log_section "Prepareconfigfile"
    
    local conf_src="${PROJECT_ROOT}/examples/test/conf/base"
    
    if [ -d "$conf_src" ]; then
        for file in "$conf_src"/*.yaml; do
            if [ -f "$file" ]; then
                cp "$file" "$CONFIG_DIR/"
                log_info "copy $(basename "$file")"
            fi
        done
        log_success "configPreparecompleted"
    else
        log_info "configdirectory，Skip"
    fi
}

# =============================================================================
# Workdirectoryinfo
# =============================================================================
save_info() {
    # currentWorkdirectorypath
    echo "$WORK_DIR" > "${PROJECT_ROOT}/integration_testing/.current"
    
    # info
    cat > "${WORK_DIR}/info.txt" << EOF
Edgion Integration Testing
===========================
Started: $(date)
Work Dir: ${WORK_DIR}

Services:
  - test_server:       PID $(cat ${PID_DIR}/test_server.pid), http://127.0.0.1:${TEST_SERVER_HTTP_PORT}
  - edgion-controller: PID $(cat ${PID_DIR}/controller.pid), http://127.0.0.1:${CONTROLLER_ADMIN_PORT}
  - edgion-gateway:    PID $(cat ${PID_DIR}/gateway.pid), http://127.0.0.1:${GATEWAY_HTTP_PORT}

Logs:
  - ${LOG_DIR}/test_server.log
  - ${LOG_DIR}/controller.log
  - ${LOG_DIR}/gateway.log
  - ${LOG_DIR}/access.log

Stop: ./examples/test/scripts/utils/kill_all.sh
EOF
}

# =============================================================================
# 
# =============================================================================
main() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion TestserviceStart${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Project:  ${PROJECT_ROOT}"
    echo -e "Work Dir: ${WORK_DIR}"
    echo -e "Test Mode: ${GREEN}enabled${NC} (Both endpoint mode + metrics test)"
    
    # : Cleanupallprocess
    kill_all_processes
    
    # : Checkbinaryfile
    check_binaries
    
    # : Workdirectory
    log_section "Workdirectory"
    mkdir -p "$LOG_DIR" "$PID_DIR" "$CONFIG_DIR"
    log_success "Workdirectorycompleted: $WORK_DIR"
    
    # :  CRD schemas 
    if [ -d "${PROJECT_ROOT}/config/crd" ]; then
        cp -r "${PROJECT_ROOT}/config/crd" "$CONFIG_DIR/"
        log_success "CRD schemas completed"
    else
        log_error "CRD schemas : ${PROJECT_ROOT}/config/crd"
        exit 1
    fi
    
    # : Prepareconfigfile
    prepare_config
    
    # : Startservice
    start_test_server
    start_controller
    start_gateway
    
    # info
    save_info
    
    # completed
    log_section "Startcompleted"
    log_success "allserviceStartsuccess！"
    echo ""
    echo "Workdirectory: ${WORK_DIR}"
    echo ""
    echo "servicestatus:"
    echo "  - test_server:       http://127.0.0.1:${TEST_SERVER_HTTP_PORT} (PID: $(cat ${PID_DIR}/test_server.pid))"
    echo "  - edgion-controller: http://127.0.0.1:${CONTROLLER_ADMIN_PORT} (PID: $(cat ${PID_DIR}/controller.pid))"
    echo "  - edgion-gateway:    http://127.0.0.1:${GATEWAY_HTTP_PORT} (PID: $(cat ${PID_DIR}/gateway.pid))"
    echo ""
    echo "logdirectory: ${LOG_DIR}"
    echo "configdirectory: ${CONFIG_DIR}"
    echo ""
    echo "Stopservice: ./examples/test/scripts/utils/kill_all.sh"
    echo ""
    
    # Workdirectorypath（script）
    echo "$WORK_DIR"
}

main "$@"
