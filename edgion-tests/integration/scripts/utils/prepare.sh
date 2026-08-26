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

    # One Cargo invocation lets Cargo schedule the complete dependency graph
    # once and guarantees that --no-prepare child jobs consume a coherent set
    # of binaries from the same source revision.
    log_section "Build integration binaries"
    if ! cargo build \
        -p edgion-controller --bin edgion-controller \
        -p edgion-gateway --bin edgion-gateway \
        -p edgion-cli --bin edgion-cli \
        -p edgion-tests --bin test_server \
        -p edgion-tests --bin test_client \
        -p edgion-tests --bin test_client_direct \
        -p edgion-tests --bin resource_diff \
        -p edgion-tests --bin config_load_validator; then
        log_error "Integration binary build failed"
        exit 1
    fi
    log_success "Integration binaries built successfully"

    # NOTE: prepare.sh is build-only. conf_sync mTLS certificates are runtime
    # material and are generated per work directory by the script that creates
    # it (conf_sync_distribute_certs) — never into the repo root.

    #
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    log_section "Summary"
    echo "Duration: ${duration}s"
    log_success "allBuildsuccess!"
    exit 0
}

main "$@"
