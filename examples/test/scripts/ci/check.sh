#!/bin/bash
# =============================================================================
# Edgion CI 检查脚本
# 用于运行 fmt、clippy 和单元测试
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

# 默认选项
RUN_FMT=true
RUN_CLIPPY=true
RUN_TESTS=true
FIX_MODE=false
VERBOSE=false

# =============================================================================
# 帮助信息
# =============================================================================
usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  -f, --fmt-only      只运行 fmt 检查"
    echo "  -c, --clippy-only   只运行 clippy 检查"
    echo "  -t, --test-only     只运行单元测试"
    echo "  --fix               自动修复 fmt 和 clippy 问题"
    echo "  -v, --verbose       显示详细输出"
    echo "  -h, --help          显示帮助信息"
    echo ""
    echo "Examples:"
    echo "  $0                  # 运行所有检查"
    echo "  $0 --fix            # 运行所有检查并自动修复"
    echo "  $0 -f               # 只检查格式"
    echo "  $0 -c -v            # 只运行 clippy，显示详细输出"
}

# =============================================================================
# 日志函数
# =============================================================================
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
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
# 解析参数
# =============================================================================
parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            -f|--fmt-only)
                RUN_CLIPPY=false
                RUN_TESTS=false
                shift
                ;;
            -c|--clippy-only)
                RUN_FMT=false
                RUN_TESTS=false
                shift
                ;;
            -t|--test-only)
                RUN_FMT=false
                RUN_CLIPPY=false
                shift
                ;;
            --fix)
                FIX_MODE=true
                shift
                ;;
            -v|--verbose)
                VERBOSE=true
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                usage
                exit 1
                ;;
        esac
    done
}

# =============================================================================
# 检查函数
# =============================================================================

# 运行 cargo fmt
run_fmt() {
    log_section "Cargo Format Check"
    
    cd "$PROJECT_ROOT"
    
    if $FIX_MODE; then
        log_info "Running cargo fmt (fix mode)..."
        cargo fmt
        log_success "Format fixed"
    else
        log_info "Running cargo fmt --check..."
        if cargo fmt --check; then
            log_success "Format check passed"
            return 0
        else
            log_error "Format check failed"
            log_info "Run with --fix to auto-fix, or run: cargo fmt"
            return 1
        fi
    fi
}

# 运行 cargo clippy
run_clippy() {
    log_section "Cargo Clippy Check"
    
    cd "$PROJECT_ROOT"
    
    # 注意：不使用 --all-features，因为 TLS 后端（boringssl/openssl/rustls）互斥
    local clippy_args="--all-targets"
    
    if $FIX_MODE; then
        log_info "Running cargo clippy --fix..."
        cargo clippy $clippy_args --fix --allow-dirty --allow-staged 2>&1 || true
        log_success "Clippy fix completed"
    else
        log_info "Running cargo clippy..."
        
        local output
        if $VERBOSE; then
            if cargo clippy $clippy_args -- -D warnings; then
                log_success "Clippy check passed"
                return 0
            else
                log_error "Clippy check failed"
                return 1
            fi
        else
            # 捕获输出，只在失败时显示
            if output=$(cargo clippy $clippy_args -- -D warnings 2>&1); then
                log_success "Clippy check passed"
                return 0
            else
                log_error "Clippy check failed"
                echo "$output"
                return 1
            fi
        fi
    fi
}

# 运行单元测试
run_tests() {
    log_section "Unit Tests"
    
    cd "$PROJECT_ROOT"
    
    log_info "Running cargo test..."
    
    local test_args=""
    if $VERBOSE; then
        test_args="--verbose"
    fi
    
    if cargo test $test_args; then
        log_success "All tests passed"
        return 0
    else
        log_error "Some tests failed"
        return 1
    fi
}

# =============================================================================
# 主函数
# =============================================================================
main() {
    parse_args "$@"
    
    local start_time=$(date +%s)
    local failed=false
    
    echo ""
    echo -e "${BLUE}Edgion CI Check${NC}"
    echo -e "Project: ${PROJECT_ROOT}"
    echo -e "Mode: $(if $FIX_MODE; then echo 'Fix'; else echo 'Check'; fi)"
    
    # 运行各项检查
    if $RUN_FMT; then
        if ! run_fmt; then
            failed=true
        fi
    fi
    
    if $RUN_CLIPPY; then
        if ! run_clippy; then
            failed=true
        fi
    fi
    
    if $RUN_TESTS; then
        if ! run_tests; then
            failed=true
        fi
    fi
    
    # 总结
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    log_section "Summary"
    echo "Duration: ${duration}s"
    
    if $failed; then
        log_error "Some checks failed!"
        exit 1
    else
        log_success "All checks passed!"
        exit 0
    fi
}

main "$@"
