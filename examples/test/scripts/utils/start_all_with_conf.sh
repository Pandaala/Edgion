#!/bin/bash
# =============================================================================
# 启动所有 Edgion 测试服务并加载配置
# 启动顺序: test_server -> controller -> 证书生成 -> 配置加载 -> gateway -> 验证
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
CERTS_DIR="${SCRIPT_DIR}/../certs"

# 创建时间戳工作目录
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/testing_${TIMESTAMP}"

# 导出 WORK_DIR 供其他脚本使用
export EDGION_WORK_DIR="$WORK_DIR"

# 子目录
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"

# 配置文件
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.toml"
GATEWAY_CONFIG="${PROJECT_ROOT}/config/edgion-gateway.toml"

# 服务端口
TEST_SERVER_HTTP_PORT=30001
CONTROLLER_ADMIN_PORT=5800
# Gateway 端口使用 http 测试套件的端口（31000）
GATEWAY_HTTP_PORT=31000

# 要加载的测试套件（默认为空，表示加载全部）
SUITES=""

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
# 显示帮助
# =============================================================================
show_help() {
    echo "启动 Edgion 测试服务并加载配置"
    echo ""
    echo "用法: $0 [OPTIONS]"
    echo ""
    echo "OPTIONS:"
    echo "  --suites <list>    指定要加载的测试套件（逗号分隔）"
    echo "                     默认：加载全部 (http,grpc,tcp,udp,http-match,...)"
    echo "  -h, --help         显示帮助"
    echo ""
    echo "示例:"
    echo "  $0                          # 加载全部配置"
    echo "  $0 --suites http,https      # 只加载 http 和 https 配置"
}

# =============================================================================
# 清理所有旧进程
# =============================================================================
kill_all_processes() {
    log_section "清理所有旧进程"
    
    # 强制 kill 所有相关进程
    pkill -9 -f "edgion-controller" 2>/dev/null && log_info "已停止 edgion-controller" || true
    pkill -9 -f "edgion-gateway" 2>/dev/null && log_info "已停止 edgion-gateway" || true
    pkill -9 -f "test_server" 2>/dev/null && log_info "已停止 test_server" || true
    
    # 确保端口释放
    sleep 2
    
    # 验证端口已释放
    local ports_busy=false
    if nc -z 127.0.0.1 $TEST_SERVER_HTTP_PORT 2>/dev/null; then
        log_error "端口 $TEST_SERVER_HTTP_PORT 仍被占用"
        ports_busy=true
    fi
    if nc -z 127.0.0.1 $CONTROLLER_ADMIN_PORT 2>/dev/null; then
        log_error "端口 $CONTROLLER_ADMIN_PORT 仍被占用"
        ports_busy=true
    fi
    if nc -z 127.0.0.1 $GATEWAY_HTTP_PORT 2>/dev/null; then
        log_error "端口 $GATEWAY_HTTP_PORT 仍被占用"
        ports_busy=true
    fi
    
    if $ports_busy; then
        log_error "无法释放端口，请手动检查"
        exit 1
    fi
    
    log_success "所有旧进程已清理，端口已释放"
}

# =============================================================================
# 检查二进制文件
# =============================================================================
check_binaries() {
    log_section "检查二进制文件"
    
    local missing=false
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/edgion-controller" ]; then
        log_error "edgion-controller 未编译"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/edgion-gateway" ]; then
        log_error "edgion-gateway 未编译"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/examples/test_server" ]; then
        log_error "test_server 未编译"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/examples/resource_diff" ]; then
        log_error "resource_diff 未编译"
        missing=true
    fi
    
    if $missing; then
        log_error "请先运行 prepare.sh 编译"
        exit 1
    fi
    
    log_success "所有二进制文件就绪"
}

# =============================================================================
# 等待端口可用
# =============================================================================
wait_for_port() {
    local port=$1
    local service_name=$2
    local pid=$3
    local timeout=${4:-30}
    local elapsed=0
    
    log_info "等待 $service_name (端口 $port)..."
    
    while [ $elapsed -lt $timeout ]; do
        # 检查进程是否存活
        if ! kill -0 $pid 2>/dev/null; then
            log_error "$service_name 进程已退出 (PID: $pid)"
            return 1
        fi
        
        # 检查端口
        if nc -z 127.0.0.1 $port 2>/dev/null; then
            log_success "$service_name 端口就绪 (端口 $port)"
            return 0
        fi
        
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name 在 ${timeout}s 内未能启动"
    return 1
}

# =============================================================================
# 等待 HTTP 健康检查
# =============================================================================
wait_for_health() {
    local url=$1
    local service_name=$2
    local timeout=${3:-10}
    local elapsed=0
    
    log_info "检查 $service_name 健康状态..."
    
    while [ $elapsed -lt $timeout ]; do
        local response=$(curl -sf "$url" 2>/dev/null)
        if [ -n "$response" ]; then
            log_success "$service_name 健康检查通过"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name 健康检查失败"
    return 1
}

# =============================================================================
# 启动 test_server
# =============================================================================
start_test_server() {
    log_section "启动 test_server"
    
    # Backend TLS 证书路径
    BACKEND_CERT="${PROJECT_ROOT}/examples/test/certs/backend/server.crt"
    BACKEND_KEY="${PROJECT_ROOT}/examples/test/certs/backend/server.key"
    
    # 检查后端 TLS 证书是否存在
    local https_backend_args=""
    if [ -f "$BACKEND_CERT" ] && [ -f "$BACKEND_KEY" ]; then
        https_backend_args="--https-backend-port 30051 --cert-file $BACKEND_CERT --key-file $BACKEND_KEY"
        log_info "启用 HTTPS 后端端口 30051"
    else
        log_warning "Backend TLS 证书不存在，跳过 HTTPS 后端"
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
    
    # 等待端口
    if ! wait_for_port $TEST_SERVER_HTTP_PORT "test_server" $pid 30; then
        log_error "test_server 启动失败，查看日志: ${LOG_DIR}/test_server.log"
        tail -20 "${LOG_DIR}/test_server.log" 2>/dev/null || true
        exit 1
    fi
    
    # 健康检查
    if ! wait_for_health "http://127.0.0.1:${TEST_SERVER_HTTP_PORT}/health" "test_server" 10; then
        log_error "test_server 健康检查失败"
        exit 1
    fi
    
    log_success "test_server 启动成功 (PID: $pid)"
}

# =============================================================================
# 启动 controller
# =============================================================================
start_controller() {
    log_section "启动 edgion-controller"
    
    "${PROJECT_ROOT}/target/debug/edgion-controller" \
        -c "$CONTROLLER_CONFIG" \
        --conf-dir "$CONFIG_DIR" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        > "${LOG_DIR}/controller.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/controller.pid"
    
    # 等待端口
    if ! wait_for_port $CONTROLLER_ADMIN_PORT "edgion-controller" $pid 30; then
        log_error "edgion-controller 启动失败，查看日志: ${LOG_DIR}/controller.log"
        tail -20 "${LOG_DIR}/controller.log" 2>/dev/null || true
        exit 1
    fi
    
    # 健康检查
    if ! wait_for_health "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/health" "edgion-controller" 10; then
        log_error "edgion-controller 健康检查失败"
        exit 1
    fi
    
    log_success "edgion-controller 启动成功 (PID: $pid)"
}

# =============================================================================
# 启动 gateway
# =============================================================================
start_gateway() {
    log_section "启动 edgion-gateway"
    
    EDGION_ACCESS_LOG="${LOG_DIR}/access.log" \
    EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log" \
    "${PROJECT_ROOT}/target/debug/edgion-gateway" \
        -c "$GATEWAY_CONFIG" \
        > "${LOG_DIR}/gateway.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/gateway.pid"
    
    # 等待端口
    if ! wait_for_port $GATEWAY_HTTP_PORT "edgion-gateway" $pid 30; then
        log_error "edgion-gateway 启动失败，查看日志: ${LOG_DIR}/gateway.log"
        tail -20 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi
    
    log_success "edgion-gateway 启动成功 (PID: $pid)"
}

# =============================================================================
# 准备基础配置文件
# =============================================================================
prepare_base_config() {
    log_section "准备基础配置文件"
    
    local conf_src="${PROJECT_ROOT}/examples/test/conf/base"
    
    if [ -d "$conf_src" ]; then
        for file in "$conf_src"/*.yaml; do
            if [ -f "$file" ]; then
                cp "$file" "$CONFIG_DIR/"
                log_info "复制 $(basename "$file")"
            fi
        done
        log_success "基础配置准备完成"
    else
        log_warning "无基础配置目录: $conf_src"
    fi
}

# =============================================================================
# 生成证书
# =============================================================================
generate_certs() {
    log_section "生成测试证书"
    
    # 生成 TLS 证书
    if [ -f "${CERTS_DIR}/generate_tls_certs.sh" ]; then
        log_info "运行 generate_tls_certs.sh..."
        if bash "${CERTS_DIR}/generate_tls_certs.sh" > /dev/null 2>&1; then
            log_success "TLS 证书生成完成"
        else
            log_warning "TLS 证书已存在或生成跳过"
        fi
    fi
    
    # 生成后端 TLS 证书
    if [ -f "${CERTS_DIR}/generate_backend_certs.sh" ]; then
        log_info "运行 generate_backend_certs.sh..."
        if bash "${CERTS_DIR}/generate_backend_certs.sh" > /dev/null 2>&1; then
            log_success "后端 TLS 证书生成完成"
        else
            log_warning "后端 TLS 证书已存在或生成跳过"
        fi
    fi
    
    # 生成 mTLS 证书
    if [ -f "${CERTS_DIR}/generate_mtls_certs.sh" ]; then
        log_info "运行 generate_mtls_certs.sh..."
        if bash "${CERTS_DIR}/generate_mtls_certs.sh" > /dev/null 2>&1; then
            log_success "mTLS 证书生成完成"
        else
            log_warning "mTLS 证书已存在或生成跳过"
        fi
    fi
    
    # 生成后端 TLS 证书
    if [ -f "${CERTS_DIR}/generate_backend_certs.sh" ]; then
        log_info "运行 generate_backend_certs.sh..."
        if bash "${CERTS_DIR}/generate_backend_certs.sh" > /dev/null 2>&1; then
            log_success "Backend TLS 证书生成完成"
        else
            log_warning "Backend TLS 证书已存在或生成跳过"
        fi
    fi
}

# =============================================================================
# 获取要加载的套件列表（支持两级目录结构）
# =============================================================================
get_suites_to_load() {
    local conf_dir="${PROJECT_ROOT}/examples/test/conf"
    
    if [ -n "$SUITES" ]; then
        # 使用用户指定的套件
        echo "$SUITES" | tr ',' ' '
    else
        # 默认：扫描 conf 目录下所有子目录
        local suites=""
        
        # 处理具有两级结构的资源类型 (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute 等)
        for resource_dir in "${conf_dir}"/*; do
            if [ -d "$resource_dir" ]; then
                local resource_name=$(basename "$resource_dir")
                
                # 跳过 base 目录
                if [ "$resource_name" = "base" ]; then
                    continue
                fi
                
                # 检查是否有子目录结构
                local has_subdir=false
                for subdir in "$resource_dir"/*; do
                    if [ -d "$subdir" ]; then
                        has_subdir=true
                        local subdir_name=$(basename "$subdir")
                        
                        # 检查是否有更深一层的子目录
                        local has_deep_subdir=false
                        for deepdir in "$subdir"/*; do
                            if [ -d "$deepdir" ]; then
                                has_deep_subdir=true
                                suites="$suites ${resource_name}/${subdir_name}/$(basename "$deepdir")"
                            fi
                        done
                        
                        if ! $has_deep_subdir; then
                            suites="$suites ${resource_name}/${subdir_name}"
                        fi
                    fi
                done
                
                # 如果没有子目录，直接添加资源目录
                if ! $has_subdir; then
                    suites="$suites $resource_name"
                fi
            fi
        done
        
        echo $suites
    fi
}

# =============================================================================
# 加载测试配置
# =============================================================================
load_configs() {
    log_section "加载测试配置"
    
    local suites_to_load=$(get_suites_to_load)
    
    if [ -z "$suites_to_load" ]; then
        log_warning "没有找到要加载的测试配置"
        return 0
    fi
    
    log_info "将加载以下配置: $suites_to_load"
    
    local load_script="${SCRIPT_DIR}/load_conf.sh"
    
    if [ ! -f "$load_script" ]; then
        log_error "load_conf.sh 不存在: $load_script"
        exit 1
    fi
    
    for suite in $suites_to_load; do
        log_info "加载 $suite 配置..."
        if bash "$load_script" "$suite" > /dev/null 2>&1; then
            log_success "$suite 配置加载完成"
        else
            log_warning "$suite 配置加载失败或为空"
        fi
    done
    
    # 等待配置生效
    log_info "等待配置生效 (3s)..."
    sleep 3
    
    # 触发 Controller 配置重载确保所有资源都处理完毕
    log_info "触发配置重载..."
    curl -sf "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/reload" > /dev/null 2>&1 || true
    sleep 2
    
    log_success "所有配置加载完成"
}

# =============================================================================
# 验证资源同步
# =============================================================================
verify_sync() {
    log_section "验证资源同步"
    
    local resource_diff="${PROJECT_ROOT}/target/debug/examples/resource_diff"
    
    if [ ! -f "$resource_diff" ]; then
        log_warning "resource_diff 未找到，跳过验证"
        return 0
    fi
    
    log_info "运行 resource_diff 验证 Controller 和 Gateway 资源同步..."
    
    if "$resource_diff" \
        --controller-url "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" \
        --gateway-url "http://127.0.0.1:${GATEWAY_HTTP_PORT}" \
        > "${LOG_DIR}/resource_diff.log" 2>&1; then
        log_success "资源同步验证通过"
    else
        log_warning "资源同步验证失败，查看日志: ${LOG_DIR}/resource_diff.log"
        tail -10 "${LOG_DIR}/resource_diff.log" 2>/dev/null || true
    fi
}

# =============================================================================
# 保存工作目录信息
# =============================================================================
save_info() {
    # 保存当前工作目录路径
    mkdir -p "${PROJECT_ROOT}/integration_testing"
    echo "$WORK_DIR" > "${PROJECT_ROOT}/integration_testing/.current"
    
    # 保存环境信息
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
    # 解析参数
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
                log_error "未知选项: $1"
                show_help
                exit 1
                ;;
        esac
    done
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion 测试服务启动（含配置加载）${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Project:  ${PROJECT_ROOT}"
    echo -e "Work Dir: ${WORK_DIR}"
    if [ -n "$SUITES" ]; then
        echo -e "Suites:   ${SUITES}"
    else
        echo -e "Suites:   全部 (自动扫描)"
    fi
    
    # 第一步: 清理所有旧进程
    kill_all_processes
    
    # 第二步: 检查二进制文件
    check_binaries
    
    # 第三步: 创建工作目录
    log_section "创建工作目录"
    mkdir -p "$LOG_DIR" "$PID_DIR" "$CONFIG_DIR"
    log_success "工作目录创建完成: $WORK_DIR"
    
    # 第四步: 生成证书（必须在复制配置前，因为会生成 Secret 文件）
    generate_certs
    
    # 第五步: 准备基础配置文件（包含新生成的 Secret）
    prepare_base_config
    
    # 第六步: 启动 test_server
    start_test_server
    
    # 第七步: 启动 controller
    start_controller
    
    # 第八步: 加载测试配置
    load_configs
    
    # 第九步: 启动 gateway
    start_gateway
    
    # 第十步: 验证资源同步
    verify_sync
    
    # 保存信息
    save_info
    
    # 完成
    log_section "启动完成"
    log_success "所有服务启动成功！"
    echo ""
    echo "工作目录: ${WORK_DIR}"
    echo ""
    echo "服务状态:"
    echo "  - test_server:       http://127.0.0.1:${TEST_SERVER_HTTP_PORT} (PID: $(cat ${PID_DIR}/test_server.pid))"
    echo "  - edgion-controller: http://127.0.0.1:${CONTROLLER_ADMIN_PORT} (PID: $(cat ${PID_DIR}/controller.pid))"
    echo "  - edgion-gateway:    http://127.0.0.1:${GATEWAY_HTTP_PORT} (PID: $(cat ${PID_DIR}/gateway.pid))"
    echo ""
    echo "日志目录: ${LOG_DIR}"
    echo "配置目录: ${CONFIG_DIR}"
    echo ""
    echo "停止服务: ./examples/test/scripts/utils/kill_all.sh"
    echo ""
    
    # 返回工作目录路径（供其他脚本获取）
    echo "$WORK_DIR"
}

main "$@"
