#!/bin/bash
# =============================================================================
# Edgion CI Checkscript
# ForRun fmt、clippy 和unitTest
# =============================================================================

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# project根directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# defaultoptions
RUN_FMT=true
RUN_CLIPPY=true
RUN_TESTS=true
FIX_MODE=false
VERBOSE=false

# =============================================================================
# helpinfo
# =============================================================================
usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  -f, --fmt-only      OnlyRun fmt Check"
    echo "  -c, --clippy-only   OnlyRun clippy Check"
    echo "  -t, --test-only     OnlyRununitTest"
    echo "  --fix               autofix fmt 和 clippy issues"
    echo "  -v, --verbose       Showdetailedoutput"
    echo "  -h, --help          Showhelpinfo"
    echo ""
    echo "Examples:"
    echo "  $0                  # RunallCheck"
    echo "  $0 --fix            # RunallCheck并autofix"
    echo "  $0 -f               # OnlyCheckformat"
    echo "  $0 -c -v            # OnlyRun clippy，Showdetailedoutput"
}

# =============================================================================
# log函数
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
# Parseargs
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
# Check函数
# =============================================================================

# Run cargo fmt
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

# Run cargo clippy
run_clippy() {
    log_section "Cargo Clippy Check"
    
    cd "$PROJECT_ROOT"
    
    # Note:不use --all-features，因为 TLS after端（boringssl/openssl/rustls）mutually exclusive
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
            # captureoutput，Only在failed时Show
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

# RununitTest
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
    
    # Run各项Check
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
