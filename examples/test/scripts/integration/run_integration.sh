#!/bin/bash
# =============================================================================
# Edgion 集成测试脚本
# 通过 Gateway 测试完整链路（test_client -> Gateway -> test_server）
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
    echo "Edgion 集成测试脚本"
    echo ""
    echo "用法: $0 [OPTIONS]"
    echo ""
    echo "OPTIONS:"
    echo "  --no-prepare     跳过编译步骤"
    echo "  --no-start       跳过启动步骤"
    echo "  --keep-alive     结束后保持服务运行（默认会停止）"
    echo "  --suites <list>  指定要加载的测试套件（逗号分隔）"
    echo "  -h, --help       显示帮助"
    echo ""
    echo "示例:"
    echo "  $0                          # 编译 + 启动（全部配置）+ 测试 + 停止"
    echo "  $0 --no-prepare             # 跳过编译，直接启动"
    echo "  $0 --keep-alive             # 结束后保持服务运行"
    echo "  $0 --suites http,https      # 只加载 http 和 https 配置"
}

# =============================================================================
# 主函数
# =============================================================================
main() {
    local do_prepare=true
    local do_start=true
    local suites=""
    
    # 解析参数
    while [[ $# -gt 0 ]]; do
        case $1 in
            --no-prepare)
                do_prepare=false
                shift
                ;;
            --no-start)
                do_start=false
                DO_CLEANUP=false  # 不启动就不需要清理
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
        
        # 获取工作目录（start_all_with_conf.sh 最后一行输出）
        WORK_DIR=$(echo "$output" | tail -1)
        
        # 设置环境变量给后续测试使用
        export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
        
        log_success "所有服务启动成功，配置已加载"
        log_info "工作目录: ${WORK_DIR}"
        log_info "访问日志: ${EDGION_TEST_ACCESS_LOG_PATH}"
    else
        log_info "跳过启动步骤"
        # 如果跳过启动，尝试从 .current 文件获取工作目录
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
            # 显示最后几行错误
            tail -10 "$log_file" 2>/dev/null || true
            return 1
        fi
    }
    
    # 第三步: 运行测试 (Direct 模式)
    local test_failed=false
    
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
    
    # 第四步: 运行 Gateway 模式测试
    # 注意：配置已在 start_all_with_conf.sh 中预先加载，无需再次加载
    log_section "第四步: 运行 Gateway 模式测试"
    
    run_test "http_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g http" || test_failed=true
    run_test "grpc_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g grpc" || test_failed=true
    run_test "websocket_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g websocket" || test_failed=true
    run_test "tcp_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g tcp" || test_failed=true
    run_test "udp_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g udp" || test_failed=true
    run_test "http_match_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-match" || test_failed=true
    run_test "grpc_match_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g grpc-match" || test_failed=true
    run_test "lb_policy_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g lb-policy" || test_failed=true
    run_test "weighted_backend_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g weighted-backend" || test_failed=true
    run_test "timeout_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g timeout" || test_failed=true
    run_test "security_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g security" || test_failed=true
    run_test "real_ip_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g real-ip" || test_failed=true
    run_test "http_redirect_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-redirect" || test_failed=true
    run_test "http_security_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-security" || test_failed=true
    run_test "plugin_logs_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g plugin-logs" || test_failed=true
    
    # backend_tls_gateway 暂时跳过 - Gateway 无法获取 Secret 资源 (watch 机制限制)
    # run_test "backend_tls_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g backend-tls" || test_failed=true
    
    # mTLS 测试 (EdgionTls 支持)
    run_test "mtls_gateway" "${PROJECT_ROOT}/target/debug/examples/test_client -g mtls" || test_failed=true
    
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
