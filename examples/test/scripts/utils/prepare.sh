#!/bin/bash
# =============================================================================
# Edgion TestPreparescript
# BuildallTest（debug ）
# =============================================================================

set -e

# 
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# projectdirectory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# =============================================================================
# log
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
# Build
# =============================================================================

# Buildbinaryfile
build_binary() {
    local name=$1
    local target=$2
    
    log_info "Build $name..."
    
    if cargo build $target 2>&1; then
        log_success "$name Buildsuccess"
        return 0
    else
        log_error "$name Buildfailed"
        return 1
    fi
}

# =============================================================================
# 
# =============================================================================
main() {
    local start_time=$(date +%s)
    
    echo ""
    echo -e "${BLUE}Edgion TestPrepare - Build${NC}"
    echo -e "Project: ${PROJECT_ROOT}"
    echo -e "Mode: Debug"
    
    cd "$PROJECT_ROOT"

    # Coverage mode: inject cargo-llvm-cov instrumentation env.
    # NOTE: cargo-llvm-cov 0.8.x's show-env does NOT set CARGO_TARGET_DIR — it relies on
    # RUSTC_WRAPPER, which cargo's incremental check ignores, so a plain `cargo build`
    # after eval would reuse non-instrumented .rlib cache from target/debug/.
    # We force CARGO_TARGET_DIR=target/llvm-cov-target to isolate the build cache and
    # guarantee a clean instrumented build. start_all.sh switches BIN_DIR accordingly.
    if [ "${EDGION_COVERAGE:-0}" = "1" ]; then
        log_info "Coverage mode enabled — using cargo-llvm-cov instrumentation"
        eval $(cargo llvm-cov show-env --export-prefix 2>/dev/null)
        export CARGO_TARGET_DIR="${PROJECT_ROOT}/target/llvm-cov-target"
        # show-env's LLVM_PROFILE_FILE points to target/Edgion-*.profraw, but cargo llvm-cov
        # report only scans target/llvm-cov-target/*.profraw. Force LLVM_PROFILE_FILE into
        # the right directory so report can find runtime profraw.
        export LLVM_PROFILE_FILE="${PROJECT_ROOT}/target/llvm-cov-target/Edgion-%p-%10m.profraw"
        log_info "CARGO_TARGET_DIR=${CARGO_TARGET_DIR}"
        log_info "LLVM_PROFILE_FILE=${LLVM_PROFILE_FILE}"
    fi

    # Build Controller (exit immediately on failure)
    log_section "Build edgion-controller"
    if ! build_binary "edgion-controller" "--bin edgion-controller"; then
        log_error "Build failed! Exiting..."
        exit 1
    fi
    
    # Build Gateway
    log_section "Build edgion-gateway"
    if ! build_binary "edgion-gateway" "--bin edgion-gateway"; then
        log_error "Build failed! Exiting..."
        exit 1
    fi
    
    # Build edgion-cli
    log_section "Build edgion-cli"
    if ! build_binary "edgion-cli" "--bin edgion-cli"; then
        log_error "Build failed! Exiting..."
        exit 1
    fi
    
    # Build test_server
    log_section "Build test_server"
    if ! build_binary "test_server" "--example test_server"; then
        log_error "Build failed! Exiting..."
        exit 1
    fi
    
    # Build test_client
    log_section "Build test_client"
    if ! build_binary "test_client" "--example test_client"; then
        log_error "Build failed! Exiting..."
        exit 1
    fi
    
    # Build test_client_direct
    log_section "Build test_client_direct"
    if ! build_binary "test_client_direct" "--example test_client_direct"; then
        log_error "Build failed! Exiting..."
        exit 1
    fi
    
    # Build resource_diff
    log_section "Build resource_diff"
    if ! build_binary "resource_diff" "--example resource_diff"; then
        log_error "Build failed! Exiting..."
        exit 1
    fi
    
    # Build config_load_validator
    log_section "Build config_load_validator"
    if ! build_binary "config_load_validator" "--example config_load_validator"; then
        log_error "Build failed! Exiting..."
        exit 1
    fi
    
    # 
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    log_section "Summary"
    echo "Duration: ${duration}s"
    log_success "allBuildsuccess!"
    exit 0
}

main "$@"
