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
    echo "  -h, --help       显示帮助"
    echo ""
    echo "示例:"
    echo "  $0                   # 编译 + 启动 + 结束后停止"
    echo "  $0 --no-prepare      # 跳过编译，直接启动"
    echo "  $0 --keep-alive      # 结束后保持服务运行"
}

# =============================================================================
# 主函数
# =============================================================================
main() {
    local do_prepare=true
    local do_start=true
    
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
    
    # 第二步: 启动服务
    if $do_start; then
        log_section "第二步: 启动所有服务"
        
        # start_all.sh 会输出工作目录路径
        local output=$("${UTILS_DIR}/start_all.sh" 2>&1)
        local exit_code=$?
        
        # 显示输出
        echo "$output"
        
        if [ $exit_code -ne 0 ]; then
            log_error "启动失败"
            exit 1
        fi
        
        # 获取工作目录（start_all.sh 最后一行输出）
        local work_dir=$(echo "$output" | tail -1)
        
        log_success "所有服务启动成功"
    else
        log_info "跳过启动步骤"
    fi
    
    # 第三步: 运行测试
    log_section "第三步: 运行 HTTP 测试 (Direct 模式)"
    
    if "${PROJECT_ROOT}/target/debug/examples/test_client" http; then
        log_success "HTTP 测试通过"
    else
        log_error "HTTP 测试失败"
        exit 1
    fi
    
    # 完成
    log_section "测试完成"
    log_success "所有测试通过！"
}

main "$@"
