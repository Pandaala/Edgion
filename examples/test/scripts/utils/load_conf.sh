#!/bin/bash
# =============================================================================
# LoadTestconfig
# 支持新的两级directory结构: Resource/Item (如 HTTPRoute/Match)
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

# config
CONF_DIR="${PROJECT_ROOT}/examples/test/conf"
EDGION_CTL="${PROJECT_ROOT}/target/debug/edgion-ctl"
CONTROLLER_URL="${EDGION_CONTROLLER_URL:-http://127.0.0.1:5800}"
GATEWAY_ADMIN_URL="${EDGION_GATEWAY_ADMIN_URL:-http://127.0.0.1:5900}"

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
# helpinfo
# =============================================================================
show_help() {
    echo "LoadTestconfig（支持两级directory结构）"
    echo ""
    echo "用法: $0 [OPTIONS] <SUITE>"
    echo ""
    echo "SUITE (支持多级path):"
    echo "  base                    基础config (GatewayClass, EdgionGatewayConfig)"
    echo "  HTTPRoute               HTTPRoute allTestconfig"
    echo "  HTTPRoute/Basic         HTTPRoute 基础Test"
    echo "  HTTPRoute/Match         HTTPRoute 匹配Test"
    echo "  HTTPRoute/Backend       HTTPRoute after端相关Test (含 LBPolicy, WeightedBackend, Timeout)"
    echo "  HTTPRoute/Filters       HTTPRoute 过滤器Test (含 Redirect, Security)"
    echo "  HTTPRoute/Protocol      HTTPRoute 协议Test (含 WebSocket)"
    echo "  grpc                    gRPC Testconfig"
    echo "  tcp                     TCP Testconfig"
    echo "  udp                     UDP Testconfig"
    echo "  all                     Loadallconfig"
    echo ""
    echo "OPTIONS:"
    echo "  --verify     Loadafterverifyresourcesync"
    echo "  --wait N     Wait N 秒让configtake effect (default: 2)"
    echo "  -h, --help   Showhelp"
    echo ""
    echo "示例:"
    echo "  $0 base                      # Load基础config"
    echo "  $0 HTTPRoute/Match           # Load HTTPRoute 匹配Testconfig"
    echo "  $0 HTTPRoute/Backend         # Load HTTPRoute after端Testconfig"
    echo "  $0 --verify HTTPRoute/Basic  # Load并verify HTTPRoute 基础config"
}

# =============================================================================
# Checkservicestatus
# =============================================================================
check_services() {
    log_info "Checkservicestatus..."
    
    # Check controller health (liveness)
    if ! curl -sf "${CONTROLLER_URL}/health" > /dev/null 2>&1; then
        log_error "Controller 未Run (${CONTROLLER_URL})"
        return 1
    fi
    
    log_success "Controller Run中"
    return 0
}

# =============================================================================
# Wait for controller to be ready (ConfigServer initialized)
# =============================================================================
wait_for_ready() {
    local max_attempts=${1:-30}
    local attempt=0
    
    log_info "等待 Controller ConfigServer 就绪..."
    
    while [ $attempt -lt $max_attempts ]; do
        if curl -sf "${CONTROLLER_URL}/ready" > /dev/null 2>&1; then
            log_success "Controller ConfigServer 已就绪"
            return 0
        fi
        
        attempt=$((attempt + 1))
        if [ $attempt -lt $max_attempts ]; then
            sleep 1
        fi
    done
    
    log_error "Controller ConfigServer 未就绪 (超时 ${max_attempts}s)"
    return 1
}

# =============================================================================
# use edgion-ctl Load单个file
# 
# FileSystemWriter 会自动使用 Kind_namespace_name.yaml 格式保存，
# 因此不需要手动复制或重命名文件。
# =============================================================================
apply_file() {
    local file=$1
    local filename=$(basename "$file")
    
    # 使用 edgion-ctl apply 加载配置
    log_info "Load $filename via API..."
    if "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$file" 2>&1; then
        log_success "$filename Loadsuccess"
        return 0
    else
        log_error "$filename Loadfailed"
        return 1
    fi
}

# =============================================================================
# 递归Loaddirectory下all yaml file
# =============================================================================
load_directory_recursive() {
    local dir=$1
    local suite_name=$2
    local failed=false
    local count=0
    
    if [ ! -d "$dir" ]; then
        log_warn "directory不存在: $dir"
        return 1
    fi
    
    # 排除动态测试的 updates 和 delete 目录（仅加载 initial）
    if [[ "$dir" =~ /DynamicTest/updates ]] || [[ "$dir" =~ /DynamicTest/delete ]]; then
        log_info "Skipping dynamic update dir: $dir"
        return 0
    fi
    
    # 递归获取all yaml file，但排除 updates 和 delete 子目录
    local files=$(find "$dir" -type f \( -name "*.yaml" -o -name "*.yml" \) \
        -not -path "*/DynamicTest/updates/*" \
        -not -path "*/DynamicTest/delete/*" | sort)
    
    if [ -z "$files" ]; then
        log_warn "$suite_name: 无configfile"
        return 0
    fi
    
    for file in $files; do
        if ! apply_file "$file"; then
            failed=true
        fi
        count=$((count + 1))
    done
    
    if $failed; then
        log_error "$suite_name: partialconfigLoadfailed"
        return 1
    else
        log_success "$suite_name: $count 个configLoadcompleted"
        return 0
    fi
}

# =============================================================================
# verifyresourcesync
# =============================================================================
verify_sync() {
    log_section "verifyresourcesync"
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/examples/resource_diff" ]; then
        log_warn "resource_diff 未Build，Skipverify"
        return 0
    fi
    
    "${PROJECT_ROOT}/target/debug/examples/resource_diff" \
        --controller-url "$CONTROLLER_URL" \
        --gateway-url "$GATEWAY_ADMIN_URL"
    
    return $?
}

# =============================================================================
# 获取all可Load的 suite 列表
# =============================================================================
get_all_suites() {
    local suites="base"
    
    # HTTPRoute 下的all子directory
    for subdir in "${CONF_DIR}/HTTPRoute"/*; do
        if [ -d "$subdir" ]; then
            local name=$(basename "$subdir")
            suites="$suites HTTPRoute/$name"
        fi
    done
    
    # 其他resource类型
    for resource in grpc grpc-match tcp udp mtls security real-ip backend-tls plugins; do
        if [ -d "${CONF_DIR}/${resource}" ]; then
            suites="$suites $resource"
        fi
    done
    
    echo "$suites"
}

# =============================================================================
# 主函数
# =============================================================================
main() {
    local suites=""
    local do_verify=false
    local wait_time=2
    
    # Parseargs
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
                log_error "unknownoptions: $1"
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
        log_error "Pleasespecify要Load的config suite"
        show_help
        exit 1
    fi
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}LoadTestconfig${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Controller: ${CONTROLLER_URL}"
    echo -e "Suite:      ${suites}"
    echo ""
    
    # Check edgion-ctl
    if [ ! -f "$EDGION_CTL" ]; then
        log_error "edgion-ctl 未Build，Please先Run prepare.sh"
        exit 1
    fi
    
    # Checkservice
    if ! check_services; then
        exit 1
    fi
    
    # Wait for ConfigServer to be ready before loading configs
    if ! wait_for_ready 30; then
        exit 1
    fi
    
    # 处理 "all"
    if [ "$suites" = "all" ]; then
        suites=$(get_all_suites)
        log_info "Loadallconfig: $suites"
    fi
    
    local failed=false
    
    # Load每个 suite
    for suite in $suites; do
        log_section "Load $suite config"
        
        local suite_dir="${CONF_DIR}/${suite}"
        
        if ! load_directory_recursive "$suite_dir" "$suite"; then
            failed=true
        fi
    done
    
    # Waitconfigtake effect
    if [ $wait_time -gt 0 ]; then
        log_info "Wait ${wait_time}s 让configtake effect..."
        sleep $wait_time
    fi
    
    # verify
    if $do_verify; then
        if ! verify_sync; then
            failed=true
        fi
    fi
    
    # 结果
    log_section "completed"
    
    if $failed; then
        log_error "partialconfigLoadfailed"
        exit 1
    else
        log_success "allconfigLoadsuccess"
        exit 0
    fi
}

main "$@"
