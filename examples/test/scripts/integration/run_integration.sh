#!/bin/bash
# =============================================================================
# Edgion 集成测试脚本
# 支持两级参数: -r Resource -i Item
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
UTILS_DIR="${SCRIPT_DIR}/../utils"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# 是否在结束时清理
DO_CLEANUP=true

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

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# =============================================================================
# 清理函数
# =============================================================================
cleanup() {
    if $DO_CLEANUP; then
        log_section "清理: 停止所有服务"
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    fi
}

# =============================================================================
# 帮助信息
# =============================================================================
show_help() {
    echo "Edgion 集成测试脚本（支持两级参数）"
    echo ""
    echo "用法: $0 [OPTIONS]"
    echo ""
    echo "OPTIONS:"
    echo "  -r, --resource <TYPE>  指定资源类型 (HTTPRoute, GRPCRoute, TCPRoute, etc.)"
    echo "  -i, --item <ITEM>      指定子项 (Match, Backend, Filters, Protocol, etc.)"
    echo "  --no-prepare           跳过编译步骤"
    echo "  --no-start             跳过启动步骤"
    echo "  --keep-alive           结束后保持服务运行（默认会停止）"
    echo "  --suites <list>        指定要加载的测试套件（逗号分隔，用于配置加载）"
    echo "  -h, --help             显示帮助"
    echo ""
    echo "示例:"
    echo "  $0                                  # 运行全部集成测试"
    echo "  $0 -r HTTPRoute                     # 运行 HTTPRoute 所有测试"
    echo "  $0 -r HTTPRoute -i Match            # 运行 HTTPRoute/Match 测试"
    echo "  $0 -r HTTPRoute -i Backend          # 运行 HTTPRoute/Backend 测试"
    echo "  $0 --no-prepare -r HTTPRoute        # 跳过编译，运行 HTTPRoute 测试"
}

# =============================================================================
# 主函数
# =============================================================================
main() {
    local do_prepare=true
    local do_start=true
    local suites=""
    local resource=""
    local item=""
    
    # 解析参数
    while [[ $# -gt 0 ]]; do
        case $1 in
            -r|--resource)
                resource="$2"
                shift 2
                ;;
            -i|--item)
                item="$2"
                shift 2
                ;;
            --no-prepare)
                do_prepare=false
                shift
                ;;
            --no-start)
                do_start=false
                DO_CLEANUP=false
                shift
                ;;
            --keep-alive)
                DO_CLEANUP=false
                shift
                ;;
            --suites)
                suites="$2"
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
    
    # 设置退出时清理
    trap cleanup EXIT
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion 集成测试${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Project: ${PROJECT_ROOT}"
    if [ -n "$resource" ]; then
        echo -e "Resource: ${resource}"
        if [ -n "$item" ]; then
            echo -e "Item: ${item}"
        fi
    fi
    echo ""
    
    cd "$PROJECT_ROOT"
    
    # 第一步: 编译
    if $do_prepare; then
        log_section "第一步: 编译所有组件"
        if ! "${UTILS_DIR}/prepare.sh"; then
            log_error "编译失败"
            exit 1
        fi
        log_success "编译完成"
    else
        log_info "跳过编译步骤"
    fi
    
    # 第二步: 启动服务（包含配置加载）
    if $do_start; then
        log_section "第二步: 启动所有服务并加载配置"
        
        # 构建启动命令
        local start_cmd="${UTILS_DIR}/start_all_with_conf.sh"
        if [ -n "$suites" ]; then
            start_cmd="$start_cmd --suites $suites"
        fi
        
        # 执行启动脚本
        local output=$($start_cmd 2>&1)
        local exit_code=$?
        
        # 显示输出
        echo "$output"
        
        if [ $exit_code -ne 0 ]; then
            log_error "启动失败"
            exit 1
        fi
        
        # 获取工作目录
        WORK_DIR=$(echo "$output" | tail -1)
        
        # 设置环境变量
        export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
        
        log_success "所有服务启动成功，配置已加载"
        log_info "工作目录: ${WORK_DIR}"
    else
        log_info "跳过启动步骤"
        if [ -f "${PROJECT_ROOT}/integration_testing/.current" ]; then
            WORK_DIR=$(cat "${PROJECT_ROOT}/integration_testing/.current")
            export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
            log_info "使用现有工作目录: ${WORK_DIR}"
        fi
    fi
    
    # 创建测试日志目录
    TEST_LOG_DIR="${WORK_DIR}/test_logs"
    mkdir -p "$TEST_LOG_DIR"
    log_info "测试日志目录: ${TEST_LOG_DIR}"
    
    # 运行测试的辅助函数
    run_test() {
        local name=$1
        local cmd=$2
        local log_file="${TEST_LOG_DIR}/${name}.log"
        
        log_info "运行 ${name} 测试..."
        if $cmd > "$log_file" 2>&1; then
            log_success "${name} 测试通过"
            return 0
        else
            log_error "${name} 测试失败 (日志: ${log_file})"
            tail -10 "$log_file" 2>/dev/null || true
            return 1
        fi
    }
    
    local test_failed=false
    
    # 根据 resource 和 item 决定运行哪些测试
    if [ -n "$resource" ]; then
        # 运行指定资源的测试
        log_section "运行 ${resource}${item:+/$item} 测试"
        
        case "$resource" in
            HTTPRoute)
                if [ -z "$item" ]; then
                    # 运行 HTTPRoute 全部测试
                    run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Basic" || test_failed=true
                    run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Match" || test_failed=true
                    run_test "HTTPRoute_Backend" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend" || test_failed=true
                    run_test "HTTPRoute_Filters" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Filters" || test_failed=true
                    run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Protocol/WebSocket" || test_failed=true
                else
                    # 运行指定子项测试
                    run_test "HTTPRoute_${item}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i ${item}" || test_failed=true
                fi
                ;;
            GRPCRoute)
                if [ -z "$item" ]; then
                    run_test "GRPCRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Basic" || test_failed=true
                    run_test "GRPCRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Match" || test_failed=true
                else
                    run_test "GRPCRoute_${item}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i ${item}" || test_failed=true
                fi
                ;;
            TCPRoute)
                run_test "TCPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i Basic" || test_failed=true
                ;;
            UDPRoute)
                run_test "UDPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r UDPRoute -i Basic" || test_failed=true
                ;;
            Gateway)
                if [ -z "$item" ]; then
                    run_test "Gateway_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Security" || test_failed=true
                    run_test "Gateway_RealIP" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i RealIP" || test_failed=true
                    run_test "Gateway_Plugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Plugins" || test_failed=true
                else
                    run_test "Gateway_${item}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i ${item}" || test_failed=true
                fi
                ;;
            EdgionTls)
                if [ -z "$item" ]; then
                    run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i https" || test_failed=true
                    run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i grpctls" || test_failed=true
                    run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i mTLS" || test_failed=true
                else
                    run_test "EdgionTls_${item}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i ${item}" || test_failed=true
                fi
                ;;
            *)
                log_error "未知资源类型: $resource"
                exit 1
                ;;
        esac
    else
        # 运行全部测试
        log_section "第三步: 运行 Direct 模式测试"
        
        run_test "http_direct" "${PROJECT_ROOT}/target/debug/examples/test_client http" || test_failed=true
        run_test "grpc_direct" "${PROJECT_ROOT}/target/debug/examples/test_client grpc" || test_failed=true
        run_test "websocket_direct" "${PROJECT_ROOT}/target/debug/examples/test_client websocket" || test_failed=true
        run_test "tcp_direct" "${PROJECT_ROOT}/target/debug/examples/test_client tcp" || test_failed=true
        run_test "udp_direct" "${PROJECT_ROOT}/target/debug/examples/test_client udp" || test_failed=true
        
        if $test_failed; then
            log_error "Direct 模式测试失败"
            log_info "查看详细日志: ${TEST_LOG_DIR}"
            exit 1
        fi
        
        log_section "第四步: 运行 Gateway 模式测试"
        
        # HTTPRoute 测试
        run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g http" || test_failed=true
        run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-match" || test_failed=true
        run_test "HTTPRoute_Backend_LBPolicy" "${PROJECT_ROOT}/target/debug/examples/test_client -g lb-policy" || test_failed=true
        run_test "HTTPRoute_Backend_WeightedBackend" "${PROJECT_ROOT}/target/debug/examples/test_client -g weighted-backend" || test_failed=true
        run_test "HTTPRoute_Backend_Timeout" "${PROJECT_ROOT}/target/debug/examples/test_client -g timeout" || test_failed=true
        run_test "HTTPRoute_Filters_Redirect" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-redirect" || test_failed=true
        run_test "HTTPRoute_Filters_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-security" || test_failed=true
        run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/examples/test_client -g websocket" || test_failed=true
        
        # GRPCRoute 测试
        run_test "GRPCRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Basic" || test_failed=true
        run_test "GRPCRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Match" || test_failed=true
        
        # TCPRoute 测试
        run_test "TCPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i Basic" || test_failed=true
        
        # UDPRoute 测试
        run_test "UDPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r UDPRoute -i Basic" || test_failed=true
        
        # Gateway 测试
        run_test "Gateway_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Security" || test_failed=true
        run_test "Gateway_RealIP" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i RealIP" || test_failed=true
        run_test "Gateway_Plugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Plugins" || test_failed=true
        
        # EdgionTls 测试
        run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i https" || test_failed=true
        run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i grpctls" || test_failed=true
        run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i mTLS" || test_failed=true
    fi
    
    if $test_failed; then
        log_error "部分测试失败"
        log_info "查看详细日志: ${TEST_LOG_DIR}"
        exit 1
    fi
    
    # 完成
    log_section "测试完成"
    log_success "所有测试通过！"
}

main "$@"
