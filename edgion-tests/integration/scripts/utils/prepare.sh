#!/bin/bash
# =============================================================================
# Edgion TestPreparescript
# BuildallTest（debug ）
# =============================================================================

set -eo pipefail

# 
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# projectdirectory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
FULL_TEST=false

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

    while [ $# -gt 0 ]; do
        case "$1" in
            --full-test)
                FULL_TEST=true
                shift
                ;;
            -h|--help)
                echo "Usage: $0 [--full-test]"
                return 0
                ;;
            *)
                log_error "Unknown option: $1"
                echo "Usage: $0 [--full-test]" >&2
                return 2
                ;;
        esac
    done
    
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
    else
        # Runtime scripts resolve artifacts from the project-local target tree.
        # Ignore an inherited custom target dir so producer and consumers cannot
        # silently disagree about the prepared artifact set.
        export CARGO_TARGET_DIR="${PROJECT_ROOT}/target"
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
        -p edgion-tests --bin config_load_validator \
        -p edgion-tests --bin cache_residue; then
        log_error "Integration binary build failed"
        exit 1
    fi
    log_success "Integration binaries built successfully"

    # Precompile the ignored real-Redis integration test and copy the hashed
    # Cargo test artifact to a stable execution-phase path. Runtime scripts
    # must never invoke Cargo after services or test scenarios have started.
    log_section "Build ignored integration test executables"
    local target_root="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"
    local ignored_test
    for ignored_test in credential_reservation_redis token_bucket_partial_refill_redis; do
        local ignored_test_bin
        if ! ignored_test_bin=$(cargo test \
            -p edgion-tests \
            --test "$ignored_test" \
            --no-run \
            --message-format=json-render-diagnostics \
            | python3 -c '
import json
import sys

wanted = sys.argv[1]
executables = []
for line in sys.stdin:
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        continue
    target = event.get("target") or {}
    executable = event.get("executable")
    if event.get("reason") == "compiler-artifact" and target.get("name") == wanted and executable:
        executables.append(executable)
if not executables:
    raise SystemExit(1)
print(executables[-1])
' "$ignored_test"); then
            log_error "Ignored integration test compilation failed: ${ignored_test}"
            exit 1
        fi
        if [ ! -x "$ignored_test_bin" ]; then
            log_error "Ignored integration test executable is missing: ${ignored_test_bin}"
            exit 1
        fi
        local stable_ignored_test_bin="${target_root}/debug/${ignored_test}"
        cp -f "$ignored_test_bin" "$stable_ignored_test_bin"
        chmod 755 "$stable_ignored_test_bin"
        log_success "Ignored integration test executable prepared: ${stable_ignored_test_bin}"
    done

    if $FULL_TEST; then
        log_section "Build external ExtProc fixture"
        local fixture_dir="${PROJECT_ROOT}/edgion-tests/integration/fixtures/ext_proc_external"
        local fixture_bin="${target_root}/debug/ext_proc_external_processor"
        if [ ! -f "${fixture_dir}/go.mod" ] || [ ! -f "${fixture_dir}/go.sum" ]; then
            log_error "External ExtProc fixture module is incomplete: ${fixture_dir}"
            exit 1
        fi
        if ! command -v go >/dev/null 2>&1; then
            log_error "Go 1.24 or newer is required for --full-test ExtProc coverage"
            exit 1
        fi

        local go_version
        if ! go_version=$(GOTOOLCHAIN=local go env GOVERSION 2>/dev/null); then
            log_error "Unable to inspect the local Go toolchain (automatic toolchain downloads are disabled)"
            exit 1
        fi
        local go_major go_minor
        go_major=$(printf '%s\n' "$go_version" | sed -n 's/^go\([0-9][0-9]*\)\.\([0-9][0-9]*\).*$/\1/p')
        go_minor=$(printf '%s\n' "$go_version" | sed -n 's/^go\([0-9][0-9]*\)\.\([0-9][0-9]*\).*$/\2/p')
        if [ -z "$go_major" ] || [ -z "$go_minor" ] \
            || [ "$go_major" -lt 1 ] \
            || { [ "$go_major" -eq 1 ] && [ "$go_minor" -lt 24 ]; }; then
            log_error "Go 1.24 or newer is required; local toolchain reports ${go_version:-unknown}"
            exit 1
        fi

        if ! (cd "$fixture_dir" && GOTOOLCHAIN=local go mod verify); then
            log_error "External ExtProc fixture module verification failed"
            exit 1
        fi
        if ! (cd "$fixture_dir" && GOTOOLCHAIN=local go test -mod=readonly ./...); then
            log_error "External ExtProc fixture tests failed"
            exit 1
        fi
        if ! (cd "$fixture_dir" && GOTOOLCHAIN=local go build -mod=readonly -trimpath -o "$fixture_bin" .); then
            log_error "External ExtProc fixture build failed"
            exit 1
        fi
        if [ ! -x "$fixture_bin" ]; then
            log_error "External ExtProc fixture binary is missing: ${fixture_bin}"
            exit 1
        fi
        log_success "External ExtProc fixture prepared: ${fixture_bin}"
    fi

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
