#!/bin/bash
# =============================================================================
# Startall Edgion Testservice并Loadconfig
# Start顺序: test_server -> controller -> 基础配置 -> 测试配置 -> gateway -> verify
# 
# 配置通过 Admin API (edgion-ctl apply) 加载，FileSystemWriter 会自动以
# Kind_namespace_name.yaml 格式保存，避免同名文件覆盖问题。
# =============================================================================

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# project根directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
CERTS_DIR="${SCRIPT_DIR}/../certs"

# 创建时间戳Workdirectory
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/testing_${TIMESTAMP}"

# 导出 WORK_DIR 供其他scriptuse
export EDGION_WORK_DIR="$WORK_DIR"

# 子directory
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"

# configfile
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.toml"
GATEWAY_CONFIG="${PROJECT_ROOT}/config/edgion-gateway.toml"

# serviceport
TEST_SERVER_HTTP_PORT=30001
CONTROLLER_ADMIN_PORT=5800
# Gateway portuse http Testsuite的port（31000）
GATEWAY_HTTP_PORT=31000
GATEWAY_ADMIN_PORT=5900

# 要Load的Testsuite（default为空，表示Loadall）
SUITES=""

# =============================================================================
# log函数
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

log_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# =============================================================================
# Showhelp
# =============================================================================
show_help() {
    echo "Start Edgion Testservice并Loadconfig"
    echo ""
    echo "用法: $0 [OPTIONS]"
    echo ""
    echo "OPTIONS:"
    echo "  --suites <list>    specify要Load的Testsuite（comma separated）"
    echo "                     default：Loadall (http,grpc,tcp,udp,http-match,...)"
    echo "  -h, --help         Showhelp"
    echo ""
    echo "示例:"
    echo "  $0                          # Loadallconfig"
    echo "  $0 --suites http,https      # OnlyLoad http 和 https config"
}

# =============================================================================
# Cleanupall旧process
# =============================================================================
kill_all_processes() {
    log_section "Cleanupall旧process"
    
    # 强制 kill all相关process
    pkill -9 -f "edgion-controller" 2>/dev/null && log_info "alreadyStop edgion-controller" || true
    pkill -9 -f "edgion-gateway" 2>/dev/null && log_info "alreadyStop edgion-gateway" || true
    pkill -9 -f "test_server" 2>/dev/null && log_info "alreadyStop test_server" || true
    
    # 确保portrelease
    sleep 2
    
    # verifyportalreadyrelease
    local ports_busy=false
    if nc -z 127.0.0.1 $TEST_SERVER_HTTP_PORT 2>/dev/null; then
        log_error "port $TEST_SERVER_HTTP_PORT 仍occupied"
        ports_busy=true
    fi
    if nc -z 127.0.0.1 $CONTROLLER_ADMIN_PORT 2>/dev/null; then
        log_error "port $CONTROLLER_ADMIN_PORT 仍occupied"
        ports_busy=true
    fi
    if nc -z 127.0.0.1 $GATEWAY_HTTP_PORT 2>/dev/null; then
        log_error "port $GATEWAY_HTTP_PORT 仍occupied"
        ports_busy=true
    fi
    
    if $ports_busy; then
        log_error "无法releaseport，Please手动Check"
        exit 1
    fi
    
    log_success "all旧processalreadyCleanup，portalreadyrelease"
}

# =============================================================================
# Checkbinaryfile
# =============================================================================
check_binaries() {
    log_section "Checkbinaryfile"
    
    local missing=false
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/edgion-controller" ]; then
        log_error "edgion-controller 未Build"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/edgion-gateway" ]; then
        log_error "edgion-gateway 未Build"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/examples/test_server" ]; then
        log_error "test_server 未Build"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/examples/resource_diff" ]; then
        log_error "resource_diff 未Build"
        missing=true
    fi
    
    if $missing; then
        log_error "Please先Run prepare.sh Build"
        exit 1
    fi
    
    log_success "allbinaryfileready"
}

# =============================================================================
# Waitport可用
# =============================================================================
wait_for_port() {
    local port=$1
    local service_name=$2
    local pid=$3
    local timeout=${4:-30}
    local elapsed=0
    
    log_info "Wait $service_name (port $port)..."
    
    while [ $elapsed -lt $timeout ]; do
        # Checkprocess是否存活
        if ! kill -0 $pid 2>/dev/null; then
            log_error "$service_name processalready退出 (PID: $pid)"
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
    
    log_error "$service_name 在 ${timeout}s 内未能Start"
    return 1
}

# =============================================================================
# Wait HTTP healthCheck (liveness)
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
# Wait for readiness check (ConfigServer ready)
# =============================================================================
wait_for_ready() {
    local url=$1
    local service_name=$2
    local timeout=${3:-30}
    local elapsed=0
    
    log_info "等待 $service_name ConfigServer 就绪..."
    
    while [ $elapsed -lt $timeout ]; do
        if curl -sf "$url" >/dev/null 2>&1; then
            log_success "$service_name ConfigServer 就绪"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name ConfigServer 未就绪 (超时 ${timeout}s)"
    return 1
}

# =============================================================================
# Start test_server
# =============================================================================
start_test_server() {
    log_section "Start test_server"
    
    # Backend TLS certificatepath
    BACKEND_CERT="${PROJECT_ROOT}/examples/test/certs/backend/server.crt"
    BACKEND_KEY="${PROJECT_ROOT}/examples/test/certs/backend/server.key"
    
    # Checkafter端 TLS certificate是否存在
    local https_backend_args=""
    if [ -f "$BACKEND_CERT" ] && [ -f "$BACKEND_KEY" ]; then
        https_backend_args="--https-backend-port 30051 --cert-file $BACKEND_CERT --key-file $BACKEND_KEY"
        log_info "启用 HTTPS after端port 30051"
    else
        log_warning "Backend TLS certificate不存在，Skip HTTPS after端"
    fi
    
    "${PROJECT_ROOT}/target/debug/examples/test_server" \
        --http-ports "30001,30002,30003" \
        --grpc-ports "30021,30022,30023" \
        --websocket-port 30005 \
        --tcp-port 30010 \
        --udp-port 30011 \
        --log-level info \
        $https_backend_args \
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
    
    # Start controller with --test-mode to enable:
    # - Both endpoint mode (sync both Endpoints and EndpointSlice)
    # - Metrics test features (test_key, test_data)
    "${PROJECT_ROOT}/target/debug/edgion-controller" \
        -c "$CONTROLLER_CONFIG" \
        --work-dir "${WORK_DIR}" \
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
    local gateway_rust_log="info,pingora_proxy=error,pingora_core=error"
    
    EDGION_ACCESS_LOG="${LOG_DIR}/access.log" \
    RUST_LOG="${gateway_rust_log}" \
    EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log" \
    "${PROJECT_ROOT}/target/debug/edgion-gateway" \
        -c "$GATEWAY_CONFIG" \
        --work-dir "${WORK_DIR}" \
        > "${LOG_DIR}/gateway.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/gateway.pid"
    
    # Wait for Gateway Admin port (always 5900, regardless of test suite listener ports)
    if ! wait_for_port $GATEWAY_ADMIN_PORT "edgion-gateway" $pid 30; then
        log_error "edgion-gateway Startfailed，viewlog: ${LOG_DIR}/gateway.log"
        tail -20 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi
    
    # Wait for Gateway to be fully ready (all caches synced from Controller)
    if ! wait_for_ready "http://127.0.0.1:${GATEWAY_ADMIN_PORT}/ready" "edgion-gateway" 60; then
        log_error "edgion-gateway 缓存同步超时，viewlog: ${LOG_DIR}/gateway.log"
        tail -30 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi
    
    # Verify LB preload completed (with retry to handle log flush race condition)
    local lb_timeout=15
    local lb_waited=0
    while ! grep -q "LB preload completed" "${LOG_DIR}/gateway.log" 2>/dev/null; do
        if [ $lb_waited -ge $lb_timeout ]; then
            log_error "edgion-gateway LB preload timeout after ${lb_timeout}s"
            tail -50 "${LOG_DIR}/gateway.log" 2>/dev/null || true
            exit 1
        fi
        sleep 1
        ((lb_waited++))
    done
    log_info "LB preload 日志验证通过 (waited ${lb_waited}s)"
    
    log_success "edgion-gateway Startsuccess (PID: $pid)"
}

# =============================================================================
# 加载基础配置文件
# 使用 edgion-ctl apply 通过 Admin API 加载，FileSystemWriter 会自动
# 以 Kind_namespace_name.yaml 格式保存到 config 目录
# =============================================================================
load_base_config() {
    log_section "加载基础配置文件"
    
    local conf_src="${PROJECT_ROOT}/examples/test/conf/base"
    local edgion_ctl="${PROJECT_ROOT}/target/debug/edgion-ctl"
    
    if [ ! -d "$conf_src" ]; then
        log_warning "无基础configdirectory: $conf_src"
        return 0
    fi
    
    for file in "$conf_src"/*.yaml; do
        if [ -f "$file" ]; then
            local filename=$(basename "$file")
            log_info "Load $filename via API..."
            if "$edgion_ctl" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" apply -f "$file" > /dev/null 2>&1; then
                log_success "$filename Loadsuccess"
            else
                log_warning "$filename Loadfailed"
            fi
        fi
    done
    
    log_success "基础config加载completed"
}

# =============================================================================
# Generatecertificate
# =============================================================================
generate_certs() {
    log_section "GenerateTestcertificate"
    
    # Generate TLS certificate
    if [ -f "${CERTS_DIR}/generate_tls_certs.sh" ]; then
        log_info "Run generate_tls_certs.sh..."
        if bash "${CERTS_DIR}/generate_tls_certs.sh" > /dev/null 2>&1; then
            log_success "TLS certificateGeneratecompleted"
        else
            log_warning "TLS certificatealready存在或GenerateSkip"
        fi
    fi
    
    # Generateafter端 TLS certificate
    if [ -f "${CERTS_DIR}/generate_backend_certs.sh" ]; then
        log_info "Run generate_backend_certs.sh..."
        if bash "${CERTS_DIR}/generate_backend_certs.sh" > /dev/null 2>&1; then
            log_success "after端 TLS certificateGeneratecompleted"
        else
            log_warning "after端 TLS certificatealready存在或GenerateSkip"
        fi
    fi
    
    # Generate mTLS certificate
    if [ -f "${CERTS_DIR}/generate_mtls_certs.sh" ]; then
        log_info "Run generate_mtls_certs.sh..."
        if bash "${CERTS_DIR}/generate_mtls_certs.sh" > /dev/null 2>&1; then
            log_success "mTLS certificateGeneratecompleted"
        else
            log_warning "mTLS certificatealready存在或GenerateSkip"
        fi
    fi
    
    # Generateafter端 TLS certificate
    if [ -f "${CERTS_DIR}/generate_backend_certs.sh" ]; then
        log_info "Run generate_backend_certs.sh..."
        if bash "${CERTS_DIR}/generate_backend_certs.sh" > /dev/null 2>&1; then
            log_success "Backend TLS certificateGeneratecompleted"
        else
            log_warning "Backend TLS certificatealready存在或GenerateSkip"
        fi
    fi
}

# =============================================================================
# 获取要Load的suite列表（支持两级directory结构）
# =============================================================================
get_suites_to_load() {
    local conf_dir="${PROJECT_ROOT}/examples/test/conf"
    
    if [ -n "$SUITES" ]; then
        # use用户specify的suite
        echo "$SUITES" | tr ',' ' '
    else
        # default：扫描 conf directory下all子directory
        local suites=""
        
        # 处理具有两级结构的resource类型 (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute 等)
        for resource_dir in "${conf_dir}"/*; do
            if [ -d "$resource_dir" ]; then
                local resource_name=$(basename "$resource_dir")
                
                # Skip base directory
                if [ "$resource_name" = "base" ]; then
                    continue
                fi
                
                # Check是否有子directory结构
                local has_subdir=false
                for subdir in "$resource_dir"/*; do
                    if [ -d "$subdir" ]; then
                        has_subdir=true
                        local subdir_name=$(basename "$subdir")
                        
                        # Check是否有更深一层的子directory
                        local has_deep_subdir=false
                        for deepdir in "$subdir"/*; do
                            if [ -d "$deepdir" ]; then
                                local deepdir_name=$(basename "$deepdir")
                                # Skip DynamicTest/updates and DynamicTest/delete
                                if [[ "$subdir_name" == "DynamicTest" && ("$deepdir_name" == "updates" || "$deepdir_name" == "delete") ]]; then
                                    continue
                                fi
                                has_deep_subdir=true
                                suites="$suites ${resource_name}/${subdir_name}/${deepdir_name}"
                            fi
                        done
                        
                        if ! $has_deep_subdir; then
                            suites="$suites ${resource_name}/${subdir_name}"
                        fi
                    fi
                done
                
                # 如果没有子directory，直接添加resourcedirectory
                if ! $has_subdir; then
                    suites="$suites $resource_name"
                fi
            fi
        done
        
        echo $suites
    fi
}

# =============================================================================
# LoadTestconfig
# =============================================================================
load_configs() {
    log_section "LoadTestconfig"
    
    local suites_to_load=$(get_suites_to_load)
    
    if [ -z "$suites_to_load" ]; then
        log_warning "没有找到要Load的Testconfig"
        return 0
    fi
    
    log_info "将Load以下config: $suites_to_load"
    
    local load_script="${SCRIPT_DIR}/load_conf.sh"
    
    if [ ! -f "$load_script" ]; then
        log_error "load_conf.sh 不存在: $load_script"
        exit 1
    fi
    
    for suite in $suites_to_load; do
        log_info "Load $suite config..."
        # 使用 --wait 0 跳过每个 suite 的等待，最后统一等待一次
        if bash "$load_script" --wait 0 "$suite" 2>&1 | tee -a "${LOG_DIR}/load_config.log"; then
            log_success "$suite configLoadcompleted"
        else
            log_warning "$suite configLoadfailed或为空"
        fi
    done
    
    # 所有配置加载完成后，等待一次即可（Controller 会自动监听目录变化）
    log_info "Waitconfigtake effect (2s)..."
    sleep 2
    
    log_success "allconfigLoadcompleted"
}

# =============================================================================
# verifyresourcesync
# =============================================================================
verify_sync() {
    log_section "verifyresourcesync"
    
    local resource_diff="${PROJECT_ROOT}/target/debug/examples/resource_diff"
    
    if [ ! -f "$resource_diff" ]; then
        log_warning "resource_diff 未找到，Skipverify"
        return 0
    fi
    
    log_info "Run resource_diff verify Controller 和 Gateway resourcesync..."
    
    # Retry logic: wait for gateway to fully sync all resources from controller
    # Gateway needs time to fetch data from controller via gRPC
    local max_retries=5
    local retry_delay=2
    local attempt=1
    
    while [ $attempt -le $max_retries ]; do
        # Note: resource_diff now skips ReferenceGrant and Secret by default (--skip-kinds)
        if "$resource_diff" \
            --controller-url "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" \
            --gateway-url "http://127.0.0.1:${GATEWAY_ADMIN_PORT}" \
            > "${LOG_DIR}/resource_diff.log" 2>&1; then
            log_success "resourcesyncverifypassed"
            return 0
        fi
        
        if [ $attempt -lt $max_retries ]; then
            log_info "verify attempt $attempt failed, retrying in ${retry_delay}s..."
            sleep $retry_delay
        fi
        ((attempt++))
    done
    
    log_error "resourcesyncverifyfailed after $max_retries attempts，viewlog: ${LOG_DIR}/resource_diff.log"
    tail -20 "${LOG_DIR}/resource_diff.log" 2>/dev/null || true
    exit 1
}

# =============================================================================
# 保存Workdirectoryinfo
# =============================================================================
save_info() {
    # 保存currentWorkdirectorypath
    mkdir -p "${PROJECT_ROOT}/integration_testing"
    echo "$WORK_DIR" > "${PROJECT_ROOT}/integration_testing/.current"
    
    # 保存环境info
    cat > "${WORK_DIR}/info.txt" << EOF
Edgion Integration Testing
===========================
Started: $(date)
Work Dir: ${WORK_DIR}
Suites: $(get_suites_to_load)

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
# 主函数
# =============================================================================
main() {
    # Parseargs
    while [[ $# -gt 0 ]]; do
        case $1 in
            --suites)
                SUITES="$2"
                shift 2
                ;;
            -h|--help)
                show_help
                exit 0
                ;;
            *)
                log_error "unknownoptions: $1"
                show_help
                exit 1
                ;;
        esac
    done
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion TestserviceStart（含configLoad）${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Project:  ${PROJECT_ROOT}"
    echo -e "Work Dir: ${WORK_DIR}"
    echo -e "Test Mode: ${GREEN}enabled${NC} (Both endpoint mode + metrics test)"
    if [ -n "$SUITES" ]; then
        echo -e "Suites:   ${SUITES}"
    else
        echo -e "Suites:   all (auto扫描)"
    fi
    
    # 第一步: Cleanupall旧process
    kill_all_processes
    
    # 第二步: Checkbinaryfile
    check_binaries
    
    # 第三步: 创建Workdirectory
    log_section "创建Workdirectory"
    mkdir -p "$LOG_DIR" "$PID_DIR" "$CONFIG_DIR"
    log_success "Workdirectory创建completed: $WORK_DIR"
    
    # 第三步半: 复制 CRD schemas 到工作目录
    if [ -d "${PROJECT_ROOT}/config/crd" ]; then
        cp -r "${PROJECT_ROOT}/config/crd" "$CONFIG_DIR/"
        log_success "CRD schemas 复制completed"
    else
        log_error "CRD schemas 目录不存在: ${PROJECT_ROOT}/config/crd"
        exit 1
    fi
    
    # 第四步: Generatecertificate（must在加载config前，因为willGenerate Secret file）
    generate_certs
    
    # 第五步: Start test_server
    start_test_server
    
    # 第六步: Start controller
    start_controller
    
    # 第七步: 等待 ConfigServer 就绪
    if ! wait_for_ready "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/ready" "edgion-controller" 30; then
        log_error "edgion-controller ConfigServer 未就绪"
        exit 1
    fi
    
    # 第八步: 加载基础配置文件（通过 API）
    load_base_config
    
    # 第九步: LoadTestconfig（通过 API）
    load_configs
    
    # 第十步: Start gateway
    start_gateway
    
    # 第十一步: verifyresourcesync
    verify_sync
    
    # 保存info
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
    
    # 返回Workdirectorypath（供其他script获取）
    echo "$WORK_DIR"
}

main "$@"
