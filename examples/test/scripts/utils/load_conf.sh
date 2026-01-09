#!/bin/bash
# =============================================================================
# 加载测试配置
# 使用 edgion-ctl 加载指定 suite 的配置到 controller
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

# 配置
CONF_DIR="${PROJECT_ROOT}/examples/test/conf"
EDGION_CTL="${PROJECT_ROOT}/target/debug/edgion-ctl"
CONTROLLER_URL="${EDGION_CONTROLLER_URL:-http://127.0.0.1:5800}"
GATEWAY_ADMIN_URL="${EDGION_GATEWAY_ADMIN_URL:-http://127.0.0.1:5900}"

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

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# =============================================================================
# 帮助信息
# =============================================================================
show_help() {
    echo "加载测试配置"
    echo ""
    echo "用法: $0 [OPTIONS] <SUITE>"
    echo ""
    echo "SUITE:"
    echo "  base         基础配置 (GatewayClass, Gateway, EdgionGatewayConfig)"
    echo "  http         HTTP 测试配置"
    echo "  https        HTTPS 测试配置"
    echo "  grpc         gRPC 测试配置"
    echo "  tcp          TCP 测试配置"
    echo "  udp          UDP 测试配置"
    echo "  websocket    WebSocket 测试配置"
    echo "  all          加载所有配置"
    echo ""
    echo "OPTIONS:"
    echo "  --verify     加载后验证资源同步"
    echo "  --wait N     等待 N 秒让配置生效 (默认: 2)"
    echo "  -h, --help   显示帮助"
    echo ""
    echo "示例:"
    echo "  $0 base          # 加载基础配置"
    echo "  $0 http          # 加载 HTTP 测试配置"
    echo "  $0 --verify http # 加载并验证 HTTP 配置"
}

# =============================================================================
# 检查服务状态
# =============================================================================
check_services() {
    log_info "检查服务状态..."
    
    # 检查 controller
    if ! curl -sf "${CONTROLLER_URL}/health" > /dev/null 2>&1; then
        log_error "Controller 未运行 (${CONTROLLER_URL})"
        return 1
    fi
    
    log_success "Controller 运行中"
    return 0
}

# =============================================================================
# 使用 edgion-ctl 加载单个文件
# =============================================================================
apply_file() {
    local file=$1
    local filename=$(basename "$file")
    
    log_info "加载 $filename..."
    
    if "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$file" 2>&1; then
        log_success "$filename 加载成功"
        return 0
    else
        log_error "$filename 加载失败"
        return 1
    fi
}

# =============================================================================
# 加载目录下所有 yaml 文件
# =============================================================================
load_directory() {
    local dir=$1
    local suite_name=$2
    local failed=false
    local count=0
    
    if [ ! -d "$dir" ]; then
        log_warn "目录不存在: $dir"
        return 1
    fi
    
    # 获取所有 yaml 文件
    local files=$(find "$dir" -maxdepth 1 -name "*.yaml" -o -name "*.yml" | sort)
    
    if [ -z "$files" ]; then
        log_warn "$suite_name: 无配置文件"
        return 0
    fi
    
    for file in $files; do
        if ! apply_file "$file"; then
            failed=true
        fi
        count=$((count + 1))
    done
    
    if $failed; then
        log_error "$suite_name: 部分配置加载失败"
        return 1
    else
        log_success "$suite_name: $count 个配置加载完成"
        return 0
    fi
}

# =============================================================================
# 触发配置重载
# =============================================================================
trigger_reload() {
    log_info "触发 Controller 配置重载..."
    
    if curl -sf -X POST "${CONTROLLER_URL}/api/v1/reload" > /dev/null 2>&1; then
        log_success "配置重载成功"
        return 0
    else
        log_warn "无法触发重载 (可能不需要)"
        return 0
    fi
}

# =============================================================================
# 验证资源同步
# =============================================================================
verify_sync() {
    log_section "验证资源同步"
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/examples/resource_diff" ]; then
        log_warn "resource_diff 未编译，跳过验证"
        return 0
    fi
    
    "${PROJECT_ROOT}/target/debug/examples/resource_diff" \
        --controller-url "$CONTROLLER_URL" \
        --gateway-url "$GATEWAY_ADMIN_URL"
    
    return $?
}

# =============================================================================
# 主函数
# =============================================================================
main() {
    local suites=""
    local do_verify=false
    local wait_time=2
    
    # 解析参数
    while [[ $# -gt 0 ]]; do
        case $1 in
            --verify)
                do_verify=true
                shift
                ;;
            --wait)
                wait_time=$2
                shift 2
                ;;
            -h|--help)
                show_help
                exit 0
                ;;
            -*)
                log_error "未知选项: $1"
                show_help
                exit 1
                ;;
            *)
                suites="$suites $1"
                shift
                ;;
        esac
    done
    
    suites=$(echo "$suites" | xargs)
    
    if [ -z "$suites" ]; then
        log_error "请指定要加载的配置 suite"
        show_help
        exit 1
    fi
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}加载测试配置${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Controller: ${CONTROLLER_URL}"
    echo -e "Suite:      ${suites}"
    echo ""
    
    # 检查 edgion-ctl
    if [ ! -f "$EDGION_CTL" ]; then
        log_error "edgion-ctl 未编译，请先运行 prepare.sh"
        exit 1
    fi
    
    # 检查服务
    if ! check_services; then
        exit 1
    fi
    
    # 处理 "all"
    if [ "$suites" = "all" ]; then
        suites="base http https grpc tcp udp websocket"
    fi
    
    local failed=false
    
    # 加载每个 suite
    for suite in $suites; do
        log_section "加载 $suite 配置"
        
        local suite_dir="${CONF_DIR}/${suite}"
        
        if ! load_directory "$suite_dir" "$suite"; then
            failed=true
        fi
    done
    
    # 等待配置生效
    if [ $wait_time -gt 0 ]; then
        log_info "等待 ${wait_time}s 让配置生效..."
        sleep $wait_time
    fi
    
    # 触发重载
    trigger_reload
    
    # 验证
    if $do_verify; then
        if ! verify_sync; then
            failed=true
        fi
    fi
    
    # 结果
    log_section "完成"
    
    if $failed; then
        log_error "部分配置加载失败"
        exit 1
    else
        log_success "所有配置加载成功"
        exit 0
    fi
}

main "$@"
