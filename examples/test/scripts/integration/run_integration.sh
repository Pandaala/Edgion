#!/bin/bash
# =============================================================================
# Edgion Integration test script
# Support two-level args: -r Resource -i Item
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
UTILS_DIR="${SCRIPT_DIR}/../utils"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# 是否在end时Cleanup
DO_CLEANUP=true

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

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# =============================================================================
# Cleanup函数
# =============================================================================
cleanup() {
    if $DO_CLEANUP; then
        log_section "Cleanup: Stopallservice"
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    fi
}

# =============================================================================
# helpinfo
# =============================================================================
show_help() {
    echo "Edgion Integration test script（Support two-level args）"
    echo ""
    echo "用法: $0 [OPTIONS]"
    echo ""
    echo "OPTIONS:"
    echo "  -r, --resource <TYPE>  specifyresource类型 (HTTPRoute, GRPCRoute, TCPRoute, etc.)"
    echo "  -i, --item <ITEM>      specifysub-item (Match, Backend, Filters, Protocol, etc.)"
    echo "  --no-prepare           SkipBuild步骤"
    echo "  --no-start             SkipStart步骤"
    echo "  --keep-alive           endafterkeepserviceRun（defaultwillStop）"
    echo "  --suites <list>        specify要Load的Testsuite（comma separated，ForconfigLoad）"
    echo "  -h, --help             Showhelp"
    echo ""
    echo "示例:"
    echo "  $0                                  # Runall集成Test"
    echo "  $0 -r HTTPRoute                     # Run HTTPRoute allTest"
    echo "  $0 -r HTTPRoute -i Match            # Run HTTPRoute/Match Test"
    echo "  $0 -r HTTPRoute -i Backend          # Run HTTPRoute/Backend Test"
    echo "  $0 --no-prepare -r HTTPRoute        # SkipBuild，Run HTTPRoute Test"
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
    
    # Parseargs
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
                log_error "unknownoptions: $1"
                show_help
                exit 1
                ;;
        esac
    done
    
    # 设置退出时Cleanup
    trap cleanup EXIT
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion 集成Test${NC}"
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
    
    # 第一步: Build
    if $do_prepare; then
        log_section "第一步: Buildall组件"
        if ! "${UTILS_DIR}/prepare.sh"; then
            log_error "Buildfailed"
            exit 1
        fi
        log_success "Buildcompleted"
    else
        log_info "SkipBuild步骤"
    fi
    
    # 第二步: Startservice（包含configLoad）
    if $do_start; then
        log_section "第二步: Startallservice并Loadconfig"
        
        # 构建Start命令
        local start_cmd="${UTILS_DIR}/start_all_with_conf.sh"
        if [ -n "$suites" ]; then
            start_cmd="$start_cmd --suites $suites"
        fi
        
        # executeStartscript
        local output=$($start_cmd 2>&1)
        local exit_code=$?
        
        # Showoutput
        echo "$output"
        
        if [ $exit_code -ne 0 ]; then
            log_error "Startfailed"
            exit 1
        fi
        
        # 获取Workdirectory
        WORK_DIR=$(echo "$output" | tail -1)
        
        # 设置环境变量
        export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
        
        log_success "allserviceStartsuccess，configalreadyLoad"
        log_info "Workdirectory: ${WORK_DIR}"
    else
        log_info "SkipStart步骤"
        if [ -f "${PROJECT_ROOT}/integration_testing/.current" ]; then
            WORK_DIR=$(cat "${PROJECT_ROOT}/integration_testing/.current")
            export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
            log_info "use现有Workdirectory: ${WORK_DIR}"
        fi
    fi
    
    # 创建Testlogdirectory
    TEST_LOG_DIR="${WORK_DIR}/test_logs"
    mkdir -p "$TEST_LOG_DIR"
    log_info "Testlogdirectory: ${TEST_LOG_DIR}"
    
    # RunTest的辅助函数
    run_test() {
        local name=$1
        local cmd=$2
        local log_file="${TEST_LOG_DIR}/${name}.log"
        
        log_info "Run ${name} Test..."
        if $cmd > "$log_file" 2>&1; then
            log_success "${name} Testpassed"
            return 0
        else
            log_error "${name} Testfailed (log: ${log_file})"
            tail -10 "$log_file" 2>/dev/null || true
            return 1
        fi
    }
    
    local test_failed=false
    
    # 根据 resource 和 item 决定Run哪些Test
    if [ -n "$resource" ]; then
        # Runspecifyresource的Test
        log_section "Run ${resource}${item:+/$item} Test"
        
        case "$resource" in
            HTTPRoute)
                if [ -z "$item" ]; then
                    # Run HTTPRoute allTest
                    run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Basic" || test_failed=true
                    run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Match" || test_failed=true
                    run_test "HTTPRoute_Backend" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend" || test_failed=true
                    run_test "HTTPRoute_Filters" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Filters" || test_failed=true
                    run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Protocol/WebSocket" || test_failed=true
                else
                    # Runspecifysub-itemTest
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
                log_error "unknownresource类型: $resource"
                exit 1
                ;;
        esac
    else
        # RunallTest
        log_section "第三步: Run Direct 模式Test"
        
        run_test "http_direct" "${PROJECT_ROOT}/target/debug/examples/test_client http" || test_failed=true
        run_test "grpc_direct" "${PROJECT_ROOT}/target/debug/examples/test_client grpc" || test_failed=true
        run_test "websocket_direct" "${PROJECT_ROOT}/target/debug/examples/test_client websocket" || test_failed=true
        run_test "tcp_direct" "${PROJECT_ROOT}/target/debug/examples/test_client tcp" || test_failed=true
        run_test "udp_direct" "${PROJECT_ROOT}/target/debug/examples/test_client udp" || test_failed=true
        
        if $test_failed; then
            log_error "Direct 模式Testfailed"
            log_info "viewdetailedlog: ${TEST_LOG_DIR}"
            exit 1
        fi
        
        log_section "第四步: Run Gateway 模式Test"
        
        # HTTPRoute Test
        run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g http" || test_failed=true
        run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-match" || test_failed=true
        run_test "HTTPRoute_Backend_LBPolicy" "${PROJECT_ROOT}/target/debug/examples/test_client -g lb-policy" || test_failed=true
        run_test "HTTPRoute_Backend_WeightedBackend" "${PROJECT_ROOT}/target/debug/examples/test_client -g weighted-backend" || test_failed=true
        run_test "HTTPRoute_Backend_Timeout" "${PROJECT_ROOT}/target/debug/examples/test_client -g timeout" || test_failed=true
        run_test "HTTPRoute_Filters_Redirect" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-redirect" || test_failed=true
        run_test "HTTPRoute_Filters_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-security" || test_failed=true
        run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/examples/test_client -g websocket" || test_failed=true
        
        # GRPCRoute Test
        run_test "GRPCRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Basic" || test_failed=true
        run_test "GRPCRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Match" || test_failed=true
        
        # TCPRoute Test
        run_test "TCPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i Basic" || test_failed=true
        
        # UDPRoute Test
        run_test "UDPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r UDPRoute -i Basic" || test_failed=true
        
        # Gateway Test
        run_test "Gateway_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Security" || test_failed=true
        run_test "Gateway_RealIP" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i RealIP" || test_failed=true
        run_test "Gateway_Plugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Plugins" || test_failed=true
        
        # EdgionTls Test
        run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i https" || test_failed=true
        run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i grpctls" || test_failed=true
        run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i mTLS" || test_failed=true
    fi
    
    if $test_failed; then
        log_error "partialTestfailed"
        log_info "viewdetailedlog: ${TEST_LOG_DIR}"
        exit 1
    fi
    
    # completed
    log_section "Testcompleted"
    log_success "allTestpassed！"
}

main "$@"
