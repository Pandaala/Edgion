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
# Dynamic Test function
# =============================================================================
run_dynamic_tests() {
    local stage=${1:-0}  # 0=完整，1=仅初始，2=仅更新后
    local edgion_ctl="${PROJECT_ROOT}/target/debug/edgion-ctl"
    local conf_dir="${PROJECT_ROOT}/examples/test/conf"
    local controller_url="http://127.0.0.1:5800"
    local gateway_url="http://127.0.0.1:5900"
    
    log_section "🔄 Gateway 动态性测试"
    
    if [ $stage -eq 0 ] || [ $stage -eq 1 ]; then
        log_info ">>> 阶段 1/2: 初始配置验证"
        
        # 运行初始阶段测试（验证约束生效）
        run_test "Dynamic_Initial_Phase" \
            "${PROJECT_ROOT}/target/debug/examples/test_client \
             -g -r Gateway -i Dynamic --phase initial" || test_failed=true
        
        log_success "初始阶段测试完成"
    fi
    
    if [ $stage -eq 0 ]; then
        log_section "📦 加载动态更新配置"
        
        # 1. 加载更新配置
        log_info "Applying updates from DynamicTest/updates/..."
        "${edgion_ctl}" --server "${controller_url}" \
            apply -f "${conf_dir}/Gateway/DynamicTest/updates/" || {
            log_error "Failed to apply dynamic updates"
            return 1
        }
        
        # 2. 处理删除操作
        if [ -f "${conf_dir}/Gateway/DynamicTest/delete/resources_to_delete.txt" ]; then
            log_info "Deleting resources..."
            while IFS= read -r resource; do
                [ -z "$resource" ] || [[ "$resource" =~ ^# ]] && continue
                # 解析格式: Kind/Namespace/Name
                IFS='/' read -r kind namespace name <<< "$resource"
                if [ -n "$kind" ] && [ -n "$namespace" ] && [ -n "$name" ]; then
                    log_info "Deleting: $kind/$namespace/$name"
                    "${edgion_ctl}" --server "${controller_url}" delete "$kind" "$name" -n "$namespace" || true
                fi
            done < "${conf_dir}/Gateway/DynamicTest/delete/resources_to_delete.txt"
        fi
        
        # 3. 验证资源同步（不阻止阶段2执行）
        log_info "Verifying resource synchronization..."
        run_test "Resource_Diff_After_Dynamic_Update" \
            "${PROJECT_ROOT}/target/debug/examples/resource_diff \
             --controller-url ${controller_url} \
             --gateway-url ${gateway_url}" || {
            log_info "Resource sync has some issues (non-blocking, continuing...)"
        }
        
        # 4. 等待配置生效
        log_info "Waiting 3s for configuration to take effect..."
        sleep 3
        
        log_success "动态配置加载完成"
    fi
    
    if [ $stage -eq 0 ] || [ $stage -eq 2 ]; then
        log_info ">>> 阶段 2/2: 动态更新后验证"
        
        # 运行更新后测试（验证约束变更生效）
        run_test "Dynamic_After_Update_Phase" \
            "${PROJECT_ROOT}/target/debug/examples/test_client \
             -g -r Gateway -i Dynamic --phase update" || test_failed=true
        
        log_success "更新阶段测试完成"
    fi
    
    log_section "✅ 动态性测试完成"
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
# Get server_id from controller
# =============================================================================
get_controller_server_id() {
    local controller_url="${1:-http://127.0.0.1:5800}"
    # Use the /api/v1/server-info endpoint to get server_id
    local response=$(curl -s "${controller_url}/api/v1/server-info" 2>/dev/null)
    local server_id=$(echo "$response" | grep -o '"server_id":"[^"]*"' | cut -d'"' -f4)
    if [ -z "$server_id" ]; then
        server_id="unknown"
    fi
    echo "$server_id"
}

# =============================================================================
# Get server_id from gateway (the server_id it received from controller)
# =============================================================================
get_gateway_server_id() {
    local gateway_url="${1:-http://127.0.0.1:5900}"
    # Use the /api/v1/server-info endpoint to get server_id
    local response=$(curl -s "${gateway_url}/api/v1/server-info" 2>/dev/null)
    local server_id=$(echo "$response" | grep -o '"server_id":"[^"]*"' | cut -d'"' -f4)
    if [ -z "$server_id" ]; then
        server_id="unknown"
    fi
    echo "$server_id"
}

# =============================================================================
# Trigger reload via Admin API
# =============================================================================
trigger_reload() {
    local controller_url="${1:-http://127.0.0.1:5800}"
    log_info "Triggering reload via Admin API..."
    local response=$(curl -s -X POST "${controller_url}/api/v1/reload" 2>/dev/null)
    local success=$(echo "$response" | grep -o '"success":true' || echo "")
    if [ -n "$success" ]; then
        log_success "Reload triggered successfully"
        return 0
    else
        log_error "Reload failed: $response"
        return 1
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
    echo "  --dynamic-test         Run Gateway dynamic configuration tests"
    echo "  --dynamic-stage <NUM>  Dynamic test stage: 1=initial, 2=update, 0=both (default: 0)"
    echo "  --suites <list>        Specify test suites to load (comma separated)"
    echo "  --with-reload          Run tests twice with reload in between (verify server_id changes)"
    echo "  -h, --help             Show help"
    echo ""
    echo "Examples:"
    echo "  $0                                  # Run all integration tests"
    echo "  $0 -r HTTPRoute                     # Run all HTTPRoute tests"
    echo "  $0 -r HTTPRoute -i Match            # Run HTTPRoute/Match test"
    echo "  $0 -r HTTPRoute -i Backend          # Run HTTPRoute/Backend test"
    echo "  $0 --no-prepare -r HTTPRoute        # Skip build, run HTTPRoute test"
    echo "  $0 --with-reload                    # Run all tests, reload, verify server_id changed, run again"
}

# =============================================================================
# Main function
# =============================================================================

# Global variables for test context (set by main, used by run_all_tests)
G_RESOURCE=""
G_ITEM=""
G_DYNAMIC_TEST=false
G_DYNAMIC_STAGE=0

# =============================================================================
# Run all tests function (can be called multiple times)
# =============================================================================
run_all_tests() {
    local round_name="${1:-}"
    local test_failed=false
    
    if [ -n "$round_name" ]; then
        log_section "🔄 Running tests: ${round_name}"
    fi
    
    # Decide which tests to run based on resource and item
    if [ -n "$G_RESOURCE" ]; then
        # Run specified resource tests
        log_section "Running ${G_RESOURCE}${G_ITEM:+/$G_ITEM} tests"
        
        case "$G_RESOURCE" in
            HTTPRoute)
                if [ -z "$G_ITEM" ]; then
                    # Run all HTTPRoute tests
                    run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Basic" || test_failed=true
                    run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Match" || test_failed=true
                    run_test "HTTPRoute_Backend" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend" || test_failed=true
                    run_test "HTTPRoute_Filters" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Filters" || test_failed=true
                    run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Protocol/WebSocket" || test_failed=true
                else
                    # Run specified sub-item test
                    run_test "HTTPRoute_${G_ITEM}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            GRPCRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "GRPCRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Basic" || test_failed=true
                    run_test "GRPCRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Match" || test_failed=true
                else
                    run_test "GRPCRoute_${G_ITEM}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            TCPRoute)
                run_test "TCPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i Basic" || test_failed=true
                ;;
            UDPRoute)
                run_test "UDPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r UDPRoute -i Basic" || test_failed=true
                ;;
            Gateway)
                if [ -z "$G_ITEM" ]; then
                    run_test "Gateway_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Security" || test_failed=true
                    run_test "Gateway_RealIP" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i RealIP" || test_failed=true
                    run_test "Gateway_TLS_GatewayTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/GatewayTLS" || test_failed=true
                    run_test "Gateway_ListenerHostname" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i ListenerHostname" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Same" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Same" || test_failed=true
                    run_test "Gateway_AllowedRoutes_All" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/All" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Kinds" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Kinds" || test_failed=true
                    run_test "Gateway_Combined" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Combined" || test_failed=true
                else
                    # Replace / with _ in item name for log file
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "Gateway_${item_safe}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            EdgionPlugins)
                if [ -z "$G_ITEM" ]; then
                    run_test "EdgionPlugins_DebugAccessLog" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DebugAccessLog" || test_failed=true
                    run_test "EdgionPlugins_PluginCondition" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PluginCondition" || test_failed=true
                else
                    run_test "EdgionPlugins_${G_ITEM}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            EdgionTls)
                if [ -z "$G_ITEM" ]; then
                    run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i https" || test_failed=true
                    run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i grpctls" || test_failed=true
                    run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i mTLS" || test_failed=true
                    run_test "EdgionTls_cipher" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i cipher" || test_failed=true
                else
                    run_test "EdgionTls_${G_ITEM}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            ReferenceGrant)
                run_test "ReferenceGrant_Status" "${PROJECT_ROOT}/target/debug/examples/test_client -g ref-grant-status" || test_failed=true
                ;;
            *)
                log_error "Unknown resource type: $G_RESOURCE"
                return 1
                ;;
        esac
    else
        # Run all tests
        log_section "Run Direct mode tests"
        
        run_test "http_direct" "${PROJECT_ROOT}/target/debug/examples/test_client http" || test_failed=true
        run_test "grpc_direct" "${PROJECT_ROOT}/target/debug/examples/test_client grpc" || test_failed=true
        run_test "websocket_direct" "${PROJECT_ROOT}/target/debug/examples/test_client websocket" || test_failed=true
        run_test "tcp_direct" "${PROJECT_ROOT}/target/debug/examples/test_client tcp" || test_failed=true
        run_test "udp_direct" "${PROJECT_ROOT}/target/debug/examples/test_client udp" || test_failed=true
        
        if $test_failed; then
            log_error "Direct mode tests failed"
            return 1
        fi
        
        log_section "Run Gateway mode tests"
        
        # HTTPRoute Tests
        run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g http" || test_failed=true
        run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-match" || test_failed=true
        run_test "HTTPRoute_Backend_LBRoundRobin" "${PROJECT_ROOT}/target/debug/examples/test_client -g lb-rr" || test_failed=true
        run_test "HTTPRoute_Backend_LBConsistentHash" "${PROJECT_ROOT}/target/debug/examples/test_client -g lb-ch" || test_failed=true
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
        # EdgionPlugins Tests
        run_test "EdgionPlugins_DebugAccessLog" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DebugAccessLog" || test_failed=true
        run_test "EdgionPlugins_PluginCondition" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PluginCondition" || test_failed=true
        run_test "Gateway_TLS_GatewayTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/GatewayTLS" || test_failed=true
        run_test "Gateway_ListenerHostname" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i ListenerHostname" || test_failed=true
        run_test "Gateway_AllowedRoutes_Same" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Same" || test_failed=true
        run_test "Gateway_AllowedRoutes_All" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/All" || test_failed=true
        run_test "Gateway_AllowedRoutes_Kinds" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Kinds" || test_failed=true
        run_test "Gateway_Combined" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Combined" || test_failed=true
        
        # EdgionTls Tests
        run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i https" || test_failed=true
        run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i grpctls" || test_failed=true
        run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i mTLS" || test_failed=true
        run_test "EdgionTls_cipher" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i cipher" || test_failed=true
        
        # ReferenceGrant Status Tests
        run_test "ReferenceGrant_Status" "${PROJECT_ROOT}/target/debug/examples/test_client -g ref-grant-status" || test_failed=true
        
        # Gateway Dynamic Tests (if enabled)
        if $G_DYNAMIC_TEST; then
            run_dynamic_tests $G_DYNAMIC_STAGE
        fi
    fi
    
    # Run dynamic tests for Gateway resource (if specified and enabled)
    if [ -n "$G_RESOURCE" ] && [ "$G_RESOURCE" = "Gateway" ] && $G_DYNAMIC_TEST; then
        run_dynamic_tests $G_DYNAMIC_STAGE
    fi
    
    if $test_failed; then
        return 1
    fi
    return 0
}

main() {
    local do_prepare=true
    local do_start=true
    local suites=""
    local with_reload=false
    
    # Parse args
    while [[ $# -gt 0 ]]; do
        case $1 in
            -r|--resource)
                G_RESOURCE="$2"
                shift 2
                ;;
            -i|--item)
                G_ITEM="$2"
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
            --dynamic-test)
                G_DYNAMIC_TEST=true
                shift
                ;;
            --dynamic-stage)
                G_DYNAMIC_STAGE=$2
                shift 2
                ;;
            --suites)
                suites="$2"
                shift 2
                ;;
            --with-reload)
                with_reload=true
                shift
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
    
    # Auto-infer suites from resource/item if not explicitly specified
    if [ -z "$suites" ] && [ -n "$G_RESOURCE" ] && [ -n "$G_ITEM" ]; then
        # Always include base and HTTPRoute/Basic for dependencies (Service, EndpointSlice, etc.)
        local base_suites="base,HTTPRoute/Basic"
        # EdgionPlugins needs its own base Gateway
        if [ "$G_RESOURCE" = "EdgionPlugins" ]; then
            base_suites="${base_suites},EdgionPlugins/base"
        fi
        suites="${base_suites},${G_RESOURCE}/${G_ITEM}"
    fi
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion Integration Test${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Project: ${PROJECT_ROOT}"
    if [ -n "$G_RESOURCE" ]; then
        echo -e "Resource: ${G_RESOURCE}"
        if [ -n "$G_ITEM" ]; then
            echo -e "Item: ${G_ITEM}"
        fi
    fi
    if $FULL_TEST; then
        echo -e "Mode: ${GREEN}Full Test${NC} (including slow tests)"
    else
        echo -e "Mode: ${YELLOW}Fast Test${NC} (slow tests skipped, use --full-test to include)"
    fi
    if $with_reload; then
        echo -e "Reload Test: ${GREEN}Enabled${NC} (will test reload functionality)"
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
    local controller_url="http://127.0.0.1:5800"
    
    # =============================================================================
    # Test execution with optional reload
    # =============================================================================
    if $with_reload; then
        # ========== Round 1: Initial tests ==========
        log_section "📋 Round 1: Initial tests (before reload)"
        
        local gateway_url="http://127.0.0.1:5900"
        
        # Get initial server_id from both Controller and Gateway
        local controller_id_before=$(get_controller_server_id "$controller_url")
        local gateway_id_before=$(get_gateway_server_id "$gateway_url")
        log_info "Initial Controller server_id: ${controller_id_before}"
        log_info "Initial Gateway server_id:    ${gateway_id_before}"
        
        # Run all tests (round 1)
        if ! run_all_tests "Round 1 (before reload)"; then
            test_failed=true
            log_error "Round 1 tests failed"
        else
            log_success "Round 1 tests passed"
        fi
        
        # ========== Trigger reload ==========
        log_section "🔄 Triggering reload"
        
        if ! trigger_reload "$controller_url"; then
            log_error "Failed to trigger reload"
            finalize_report
            exit 1
        fi
        
        # Wait for reload to complete and Gateway to sync
        log_info "Waiting 5 seconds for reload to complete and Gateway to sync..."
        sleep 5
        
        # ========== Verify Controller server_id changed ==========
        local controller_id_after=$(get_controller_server_id "$controller_url")
        log_info "New Controller server_id: ${controller_id_after}"
        
        if [ "$controller_id_before" = "$controller_id_after" ]; then
            log_error "Controller server_id did not change after reload!"
            log_error "Before: ${controller_id_before}"
            log_error "After:  ${controller_id_after}"
            report_test "Reload_Controller_ServerID_Changed" "FAIL" "0"
            finalize_report
            exit 1
        else
            log_success "Controller server_id changed successfully!"
            log_info "Before: ${controller_id_before}"
            log_info "After:  ${controller_id_after}"
            report_test "Reload_Controller_ServerID_Changed" "PASS" "0"
        fi
        
        # ========== Verify Gateway server_id changed ==========
        local gateway_id_after=$(get_gateway_server_id "$gateway_url")
        log_info "New Gateway server_id: ${gateway_id_after}"
        
        if [ "$gateway_id_before" = "$gateway_id_after" ]; then
            log_error "Gateway server_id did not change after reload!"
            log_error "Before: ${gateway_id_before}"
            log_error "After:  ${gateway_id_after}"
            log_error "This means Gateway did not detect the reload and re-sync!"
            report_test "Reload_Gateway_ServerID_Changed" "FAIL" "0"
            finalize_report
            exit 1
        else
            log_success "Gateway server_id changed successfully!"
            log_info "Before: ${gateway_id_before}"
            log_info "After:  ${gateway_id_after}"
            report_test "Reload_Gateway_ServerID_Changed" "PASS" "0"
        fi
        
        # ========== Verify Gateway and Controller have same server_id ==========
        if [ "$controller_id_after" != "$gateway_id_after" ]; then
            log_error "Gateway server_id does not match Controller!"
            log_error "Controller: ${controller_id_after}"
            log_error "Gateway:    ${gateway_id_after}"
            report_test "Reload_ServerID_Match" "FAIL" "0"
        else
            log_success "Gateway and Controller server_id match!"
            report_test "Reload_ServerID_Match" "PASS" "0"
        fi
        
        # ========== Clear access log before Round 2 ==========
        # Some tests (like LBPolicy) analyze access logs, so we need to clear them
        # to avoid counting requests from Round 1
        if [ -n "$EDGION_TEST_ACCESS_LOG_PATH" ] && [ -f "$EDGION_TEST_ACCESS_LOG_PATH" ]; then
            log_info "Clearing access log for Round 2..."
            > "$EDGION_TEST_ACCESS_LOG_PATH"
        fi
        
        # ========== Round 2: Tests after reload ==========
        log_section "📋 Round 2: Tests after reload"
        
        # Run all tests (round 2)
        if ! run_all_tests "Round 2 (after reload)"; then
            test_failed=true
            log_error "Round 2 tests failed"
        else
            log_success "Round 2 tests passed"
        fi
    else
        # Normal test execution (without reload)
        if ! run_all_tests; then
            test_failed=true
        fi
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
