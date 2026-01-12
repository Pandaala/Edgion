#!/bin/bash
# =============================================================================
# Edgion Integration Test Script
# Support two-level args: -r Resource -i Item
# =============================================================================

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Project root directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UTILS_DIR="${SCRIPT_DIR}/../utils"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# Whether to cleanup at end
DO_CLEANUP=true

# Whether to run full tests including slow tests (default: false for faster iteration)
FULL_TEST=false

# Report file path (will be set after WORK_DIR is determined)
REPORT_FILE=""

# Test counters
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0

# =============================================================================
# Log functions
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
# Slow test management
# =============================================================================
# Slow tests list (skipped by default, run with --full-test)
SLOW_TESTS=(
    "HTTPRoute_Backend_Timeout"
)

is_slow_test() {
    local test_name=$1
    for slow in "${SLOW_TESTS[@]}"; do
        if [[ "$test_name" == "$slow" ]]; then
            return 0
        fi
    done
    return 1
}

should_skip_test() {
    local test_name=$1
    if ! $FULL_TEST && is_slow_test "$test_name"; then
        return 0  # Should skip
    fi
    return 1  # Don't skip
}

# =============================================================================
# Report functions
# =============================================================================
init_report() {
    REPORT_FILE="${WORK_DIR}/report.log"
    echo "========================================" > "$REPORT_FILE"
    echo "Edgion Integration Test Report" >> "$REPORT_FILE"
    echo "Time: $(date '+%Y-%m-%d %H:%M:%S')" >> "$REPORT_FILE"
    echo "Work Dir: ${WORK_DIR}" >> "$REPORT_FILE"
    echo "Full Test: ${FULL_TEST}" >> "$REPORT_FILE"
    echo "========================================" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"
}

report_test() {
    local name=$1
    local result=$2
    local duration=$3
    
    ((TOTAL_TESTS++))
    if [ "$result" = "PASS" ]; then
        ((PASSED_TESTS++))
        echo "[PASS] ${name} (${duration}s)" >> "$REPORT_FILE"
    else
        ((FAILED_TESTS++))
        echo "[FAIL] ${name} (${duration}s)" >> "$REPORT_FILE"
    fi
}

finalize_report() {
    echo "" >> "$REPORT_FILE"
    echo "========================================" >> "$REPORT_FILE"
    echo "Summary" >> "$REPORT_FILE"
    echo "========================================" >> "$REPORT_FILE"
    echo "Total:  ${TOTAL_TESTS}" >> "$REPORT_FILE"
    echo "Passed: ${PASSED_TESTS}" >> "$REPORT_FILE"
    echo "Failed: ${FAILED_TESTS}" >> "$REPORT_FILE"
    if [ $TOTAL_TESTS -gt 0 ]; then
        local pass_rate=$((PASSED_TESTS * 100 / TOTAL_TESTS))
        echo "Pass Rate: ${pass_rate}%" >> "$REPORT_FILE"
    fi
    echo "========================================" >> "$REPORT_FILE"
    
    # Also print to console
    echo ""
    echo "Report saved to: ${REPORT_FILE}"
}

# =============================================================================
# Cleanup function
# =============================================================================
cleanup() {
    if $DO_CLEANUP; then
        log_section "Cleanup: Stop all services"
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    fi
}

# =============================================================================
# Help info
# =============================================================================
show_help() {
    echo "Edgion Integration Test Script (Support two-level args)"
    echo ""
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "OPTIONS:"
    echo "  -r, --resource <TYPE>  Specify resource type (HTTPRoute, GRPCRoute, TCPRoute, etc.)"
    echo "  -i, --item <ITEM>      Specify sub-item (Match, Backend, Filters, Protocol, etc.)"
    echo "  --no-prepare           Skip build step"
    echo "  --no-start             Skip start step"
    echo "  --keep-alive           Keep services running after end (default will stop)"
    echo "  --full-test            Run all tests including slow tests (timeout, etc.)"
    echo "  --suites <list>        Specify test suites to load (comma separated)"
    echo "  -h, --help             Show help"
    echo ""
    echo "Examples:"
    echo "  $0                                  # Run all integration tests"
    echo "  $0 -r HTTPRoute                     # Run all HTTPRoute tests"
    echo "  $0 -r HTTPRoute -i Match            # Run HTTPRoute/Match test"
    echo "  $0 -r HTTPRoute -i Backend          # Run HTTPRoute/Backend test"
    echo "  $0 --no-prepare -r HTTPRoute        # Skip build, run HTTPRoute test"
}

# =============================================================================
# Main function
# =============================================================================
main() {
    local do_prepare=true
    local do_start=true
    local suites=""
    local resource=""
    local item=""
    
    # Parse args
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
            --full-test)
                FULL_TEST=true
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
                log_error "Unknown option: $1"
                show_help
                exit 1
                ;;
        esac
    done
    
    # Setup cleanup on exit
    trap cleanup EXIT
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion Integration Test${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Project: ${PROJECT_ROOT}"
    if [ -n "$resource" ]; then
        echo -e "Resource: ${resource}"
        if [ -n "$item" ]; then
            echo -e "Item: ${item}"
        fi
    fi
    if $FULL_TEST; then
        echo -e "Mode: ${GREEN}Full Test${NC} (including slow tests)"
    else
        echo -e "Mode: ${YELLOW}Fast Test${NC} (slow tests skipped, use --full-test to include)"
    fi
    echo ""
    
    cd "$PROJECT_ROOT"
    
    # Step 1: Build
    if $do_prepare; then
        log_section "Step 1: Build all components"
        if ! "${UTILS_DIR}/prepare.sh"; then
            log_error "Build failed"
            exit 1
        fi
        log_success "Build completed"
    else
        log_info "Skip build step"
    fi
    
    # Step 2: Start services (including config load)
    if $do_start; then
        log_section "Step 2: Start all services and load config"
        
        # Build start command
        local start_cmd="${UTILS_DIR}/start_all_with_conf.sh"
        if [ -n "$suites" ]; then
            start_cmd="$start_cmd --suites $suites"
        fi
        
        # Execute start script
        local output=$($start_cmd 2>&1)
        local exit_code=$?
        
        # Show output
        echo "$output"
        
        if [ $exit_code -ne 0 ]; then
            log_error "Start failed"
            exit 1
        fi
        
        # Get work directory
        WORK_DIR=$(echo "$output" | tail -1)
        
        # Set environment variables
        export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
        
        log_success "All services started, config loaded"
        log_info "Work directory: ${WORK_DIR}"
    else
        log_info "Skip start step"
        if [ -f "${PROJECT_ROOT}/integration_testing/.current" ]; then
            WORK_DIR=$(cat "${PROJECT_ROOT}/integration_testing/.current")
            export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
            log_info "Using existing work directory: ${WORK_DIR}"
        fi
    fi
    
    # Create test log directory
    TEST_LOG_DIR="${WORK_DIR}/test_logs"
    mkdir -p "$TEST_LOG_DIR"
    log_info "Test log directory: ${TEST_LOG_DIR}"
    
    # Initialize report
    init_report
    log_info "Report file: ${REPORT_FILE}"
    
    # Helper function to run tests
    run_test() {
        local name=$1
        local cmd=$2
        local log_file="${TEST_LOG_DIR}/${name}.log"
        local start_time=$(date +%s)
        
        log_info "Running ${name} test..."
        if $cmd > "$log_file" 2>&1; then
            local end_time=$(date +%s)
            local duration=$((end_time - start_time))
            log_success "${name} test passed"
            report_test "$name" "PASS" "$duration"
            return 0
        else
            local end_time=$(date +%s)
            local duration=$((end_time - start_time))
            log_error "${name} test failed (log: ${log_file})"
            report_test "$name" "FAIL" "$duration"
            tail -10 "$log_file" 2>/dev/null || true
            return 1
        fi
    }
    
    local test_failed=false
    
    # Decide which tests to run based on resource and item
    if [ -n "$resource" ]; then
        # Run specified resource tests
        log_section "Running ${resource}${item:+/$item} tests"
        
        case "$resource" in
            HTTPRoute)
                if [ -z "$item" ]; then
                    # Run all HTTPRoute tests
                    run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Basic" || test_failed=true
                    run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Match" || test_failed=true
                    run_test "HTTPRoute_Backend" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend" || test_failed=true
                    run_test "HTTPRoute_Filters" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Filters" || test_failed=true
                    run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Protocol/WebSocket" || test_failed=true
                else
                    # Run specified sub-item test
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
                    run_test "EdgionTls_cipher" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i cipher" || test_failed=true
                else
                    run_test "EdgionTls_${item}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i ${item}" || test_failed=true
                fi
                ;;
            *)
                log_error "Unknown resource type: $resource"
                exit 1
                ;;
        esac
    else
        # Run all tests
        log_section "Step 3: Run Direct mode tests"
        
        run_test "http_direct" "${PROJECT_ROOT}/target/debug/examples/test_client http" || test_failed=true
        run_test "grpc_direct" "${PROJECT_ROOT}/target/debug/examples/test_client grpc" || test_failed=true
        run_test "websocket_direct" "${PROJECT_ROOT}/target/debug/examples/test_client websocket" || test_failed=true
        run_test "tcp_direct" "${PROJECT_ROOT}/target/debug/examples/test_client tcp" || test_failed=true
        run_test "udp_direct" "${PROJECT_ROOT}/target/debug/examples/test_client udp" || test_failed=true
        
        if $test_failed; then
            log_error "Direct mode tests failed"
            log_info "View detailed logs: ${TEST_LOG_DIR}"
            finalize_report
            exit 1
        fi
        
        log_section "Step 4: Run Gateway mode tests"
        
        # HTTPRoute Tests
        run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g http" || test_failed=true
        run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-match" || test_failed=true
        run_test "HTTPRoute_Backend_LBPolicy" "${PROJECT_ROOT}/target/debug/examples/test_client -g lb-policy" || test_failed=true
        run_test "HTTPRoute_Backend_WeightedBackend" "${PROJECT_ROOT}/target/debug/examples/test_client -g weighted-backend" || test_failed=true
        if ! should_skip_test "HTTPRoute_Backend_Timeout"; then
            run_test "HTTPRoute_Backend_Timeout" "${PROJECT_ROOT}/target/debug/examples/test_client -g timeout" || test_failed=true
        else
            log_info "Skipping slow test: HTTPRoute_Backend_Timeout (use --full-test to run)"
        fi
        run_test "HTTPRoute_Filters_Redirect" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-redirect" || test_failed=true
        run_test "HTTPRoute_Filters_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-security" || test_failed=true
        run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/examples/test_client -g websocket" || test_failed=true
        
        # GRPCRoute Tests
        run_test "GRPCRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Basic" || test_failed=true
        run_test "GRPCRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Match" || test_failed=true
        
        # TCPRoute Tests
        run_test "TCPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i Basic" || test_failed=true
        
        # UDPRoute Tests
        run_test "UDPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r UDPRoute -i Basic" || test_failed=true
        
        # Gateway Tests
        run_test "Gateway_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Security" || test_failed=true
        run_test "Gateway_RealIP" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i RealIP" || test_failed=true
        run_test "Gateway_Plugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Plugins" || test_failed=true
        
        # EdgionTls Tests
        run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i https" || test_failed=true
        run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i grpctls" || test_failed=true
        run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i mTLS" || test_failed=true
        run_test "EdgionTls_cipher" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i cipher" || test_failed=true
    fi
    
    # Finalize report
    finalize_report
    
    if $test_failed; then
        log_error "Some tests failed"
        log_info "View detailed logs: ${TEST_LOG_DIR}"
        exit 1
    fi
    
    # Completed
    log_section "Test completed"
    log_success "All tests passed!"
}

main "$@"
