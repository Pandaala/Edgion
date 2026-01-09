#!/bin/bash
# =============================================================================
# Edgion 测试准备脚本
# 预编译所有测试所需的组件（debug 模式）
# =============================================================================

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

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
# 编译函数
# =============================================================================

# 编译二进制文件
build_binary() {
    local name=$1
    local target=$2
    
    log_info "编译 $name..."
    
    if cargo build $target 2>&1; then
        log_success "$name 编译成功"
        return 0
    else
        log_error "$name 编译失败"
        return 1
    fi
}

# =============================================================================
# 主函数
# =============================================================================
main() {
    local start_time=$(date +%s)
    local failed=false
    
    echo ""
    echo -e "${BLUE}Edgion 测试准备 - 预编译组件${NC}"
    echo -e "Project: ${PROJECT_ROOT}"
    echo -e "Mode: Debug"
    
    cd "$PROJECT_ROOT"
    
    # 编译 Controller
    log_section "编译 edgion-controller"
    if ! build_binary "edgion-controller" "--bin edgion-controller"; then
        failed=true
    fi
    
    # 编译 Gateway
    log_section "编译 edgion-gateway"
    if ! build_binary "edgion-gateway" "--bin edgion-gateway"; then
        failed=true
    fi
    
    # 编译 edgion-ctl
    log_section "编译 edgion-ctl"
    if ! build_binary "edgion-ctl" "--bin edgion-ctl"; then
        failed=true
    fi
    
    # 编译 test_server
    log_section "编译 test_server"
    if ! build_binary "test_server" "--example test_server"; then
        failed=true
    fi
    
    # 编译 test_client
    log_section "编译 test_client"
    if ! build_binary "test_client" "--example test_client"; then
        failed=true
    fi
    
    # 编译 test_client_direct
    log_section "编译 test_client_direct"
    if ! build_binary "test_client_direct" "--example test_client_direct"; then
        failed=true
    fi
    
    # 编译 resource_diff
    log_section "编译 resource_diff"
    if ! build_binary "resource_diff" "--example resource_diff"; then
        failed=true
    fi
    
    # 编译 config_load_validator
    log_section "编译 config_load_validator"
    if ! build_binary "config_load_validator" "--example config_load_validator"; then
        failed=true
    fi
    
    # 总结
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    log_section "Summary"
    echo "Duration: ${duration}s"
    
    if $failed; then
        log_error "部分组件编译失败!"
        exit 1
    else
        log_success "所有组件编译成功!"
        exit 0
    fi
}

main "$@"
