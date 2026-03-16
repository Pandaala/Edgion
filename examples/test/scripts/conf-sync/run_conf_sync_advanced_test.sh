#!/bin/bash
# =============================================================================
# Config-Sync Advanced Leak Detection Test
#
# Self-contained: automatically starts/stops test environment.
# Tests complex scenarios that may trigger config leaks:
#   1. Orphan routes (non-existent Gateway/Service references)
#   2. Wildcard + catch-all route cleanup
#   3. Out-of-order deletion (Service before Route, Route before Service)
#   4. Rapid fire inject+delete (CompressEvent coalescing)
#   5. Duplicate apply (same config N times)
#   6. Stream plugin lifecycle (EdgionStreamPlugins + annotation)
#   7. BackendTLSPolicy lifecycle (policies + reverse_index)
#   8. Mixed lifecycle (add while deleting)
#   9. Full multi-protocol blast (all resource types at once)
#
# Usage:
#   ./run_conf_sync_advanced_test.sh                # Full run
#   ./run_conf_sync_advanced_test.sh --no-start     # Attach to running env
#   ./run_conf_sync_advanced_test.sh --keep-alive   # Don't stop after test
# =============================================================================

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UTILS_DIR="${SCRIPT_DIR}/../utils"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
CONF_DIR="${PROJECT_ROOT}/examples/test/conf/conf-sync-leak-test"
EDGION_CTL="${PROJECT_ROOT}/target/debug/edgion-ctl"

CONTROLLER_URL="${EDGION_CONTROLLER_URL:-http://127.0.0.1:5800}"
GATEWAY_ADMIN_URL="${EDGION_GATEWAY_ADMIN_URL:-http://127.0.0.1:5900}"

SYNC_WAIT=2
CLEANUP_WAIT=5
MAX_VERIFY_RETRIES=20
VERIFY_RETRY_INTERVAL=2

PASSED=0
FAILED=0
TOTAL=0

DO_START=true
DO_CLEANUP=true
BASELINE_DIR=""
BASELINE_STORE_STATS=""

# =============================================================================
# Log helpers
# =============================================================================
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error()   { echo -e "${RED}[✗]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}
log_scenario() {
    echo ""
    echo -e "${CYAN}--- Scenario: $1 ---${NC}"
}

# =============================================================================
# Environment lifecycle
# =============================================================================
start_environment() {
    log_section "Starting test environment"
    local start_cmd="${UTILS_DIR}/start_all_with_conf.sh --suites HTTPRoute/Basic"
    local output exit_code
    output=$($start_cmd 2>&1) && exit_code=0 || exit_code=$?
    echo "$output"
    if [ $exit_code -ne 0 ]; then
        log_error "Failed to start test environment"
        exit 1
    fi
    log_success "Test environment started"
}

cleanup() {
    [ -n "$BASELINE_DIR" ] && rm -rf "$BASELINE_DIR" 2>/dev/null || true
    if $DO_CLEANUP; then
        log_section "Stopping test environment"
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    fi
}

# =============================================================================
# edgion-ctl helpers
# =============================================================================
ctl_apply() {
    "$EDGION_CTL" --server "$CONTROLLER_URL" apply -f "$1" > /dev/null 2>&1
}

ctl_delete() {
    local kind=$1 name=$2 ns=${3:-edgion-test}
    "$EDGION_CTL" --server "$CONTROLLER_URL" delete "$kind" "$name" -n "$ns" > /dev/null 2>&1 || true
}

# =============================================================================
# Prerequisite checks
# =============================================================================
check_prerequisites() {
    if [ ! -f "$EDGION_CTL" ]; then log_error "edgion-ctl not found"; exit 1; fi
    if ! curl -sf "${CONTROLLER_URL}/health" > /dev/null 2>&1; then
        log_error "Controller not reachable at ${CONTROLLER_URL}"; exit 1
    fi
    if ! curl -sf "${GATEWAY_ADMIN_URL}/health" > /dev/null 2>&1; then
        log_error "Gateway admin not reachable at ${GATEWAY_ADMIN_URL}"; exit 1
    fi
    local status
    status=$(curl -sf "${GATEWAY_ADMIN_URL}/api/v1/testing/status" 2>/dev/null || echo "")
    if [ -z "$status" ]; then log_error "Gateway not in integration-testing-mode"; exit 1; fi
    log_success "Prerequisites OK"
}

# =============================================================================
# JSON field extraction
# =============================================================================
json_field() {
    local json=$1 field=$2
    echo "$json" | python3 -c "
import sys, json, functools
data = json.load(sys.stdin)
keys = '${field}'.lstrip('.').split('.')
try:
    result = functools.reduce(lambda d, k: d[k], keys, data)
    print(result)
except (KeyError, TypeError):
    print('-1')
" 2>/dev/null || echo "-1"
}

# =============================================================================
# Baseline capture and verification
# =============================================================================
capture_baseline() {
    log_info "Capturing baseline counts..."
    BASELINE_DIR=$(mktemp -d)
    local kinds=(httproute grpcroute tcproute udproute tlsroute edgiontls edgionplugins edgionstreamplugins backendtlspolicy service endpointslice)
    for kind in "${kinds[@]}"; do
        local resp count
        resp=$(curl -sf "${GATEWAY_ADMIN_URL}/configclient/${kind}/list" 2>/dev/null || echo '{"count":0}')
        count=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('count',0))" 2>/dev/null || echo "0")
        echo "$count" > "${BASELINE_DIR}/cc_${kind}"
    done
    BASELINE_STORE_STATS=$(curl -sf "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
    log_success "Baseline captured"
}

get_baseline_cc() { cat "${BASELINE_DIR}/cc_${1}" 2>/dev/null || echo "0"; }

verify_configclient_baseline() {
    local all_ok=true
    local kinds=(httproute grpcroute tcproute udproute tlsroute edgiontls edgionplugins edgionstreamplugins backendtlspolicy service endpointslice)
    for kind in "${kinds[@]}"; do
        local resp count baseline
        resp=$(curl -sf "${GATEWAY_ADMIN_URL}/configclient/${kind}/list" 2>/dev/null || echo '{"count":-1}')
        count=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('count',-1))" 2>/dev/null || echo "-1")
        baseline=$(get_baseline_cc "$kind")
        if [ "$count" != "$baseline" ]; then
            all_ok=false
            log_warn "  configclient/${kind}: count=${count} (baseline=${baseline})"
        fi
    done
    $all_ok
}

verify_store_stats_baseline() {
    local stats
    stats=$(curl -sf "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
    if [ -z "$stats" ]; then log_error "  Failed to fetch store-stats"; return 1; fi

    local all_ok=true
    local check_fields=(
        ".data.http_routes.http_routes"  ".data.http_routes.exact_domains"  ".data.http_routes.wildcard_domains"
        ".data.grpc_routes.grpc_routes"  ".data.grpc_routes.resource_keys"
        ".data.tcp_routes.routes_by_key" ".data.udp_routes.routes_by_key"
        ".data.tls_routes.route_cache"   ".data.tls_store.entries"  ".data.tls_cert_matcher.port_count"
        ".data.plugin_store.plugins"     ".data.stream_plugin_store.plugins"
        ".data.link_sys_store.resources" ".data.policy_store.total_services"
        ".data.backend_tls_policy.policies" ".data.backend_tls_policy.reverse_index_targets"
    )
    for field in "${check_fields[@]}"; do
        local current baseline
        current=$(json_field "$stats" "$field")
        baseline=$(json_field "$BASELINE_STORE_STATS" "$field")
        if [ "$current" != "$baseline" ]; then
            all_ok=false
            log_warn "  store-stats ${field} = ${current} (baseline=${baseline})"
        fi
    done

    local current_ca baseline_ca
    current_ca=$(json_field "$stats" ".data.http_routes.has_catch_all")
    baseline_ca=$(json_field "$BASELINE_STORE_STATS" ".data.http_routes.has_catch_all")
    if [ "$current_ca" != "$baseline_ca" ]; then
        all_ok=false
        log_warn "  store-stats http_routes.has_catch_all = ${current_ca} (baseline=${baseline_ca})"
    fi
    $all_ok
}

verify_all_baseline() {
    for i in $(seq 1 "$MAX_VERIFY_RETRIES"); do
        local cc_ok=true ss_ok=true
        verify_configclient_baseline || cc_ok=false
        verify_store_stats_baseline  || ss_ok=false
        if $cc_ok && $ss_ok; then return 0; fi
        if [ "$i" -lt "$MAX_VERIFY_RETRIES" ]; then
            log_info "  Retry ${i}/${MAX_VERIFY_RETRIES} — waiting ${VERIFY_RETRY_INTERVAL}s..."
            sleep "$VERIFY_RETRY_INTERVAL"
        fi
    done
    return 1
}

record_result() {
    local name=$1 result=$2
    TOTAL=$((TOTAL + 1))
    if [ "$result" = "pass" ]; then
        PASSED=$((PASSED + 1))
        log_success "Scenario '${name}': PASS"
    else
        FAILED=$((FAILED + 1))
        log_error "Scenario '${name}': FAIL — LEAK DETECTED"
        curl -sf "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null \
            | python3 -m json.tool 2>/dev/null || true
    fi
}

cleanup_all_test_resources() {
    log_info "Cleaning up all test resources..."
    local -a all_resources=(
        "HTTPRoute leak-http"       "HTTPRoute leak-wildcard"
        "HTTPRoute leak-catchall"   "HTTPRoute leak-orphan"
        "GRPCRoute leak-grpc"       "TCPRoute leak-tcp"
        "TCPRoute leak-tcp-sp"      "UDPRoute leak-udp"
        "TLSRoute leak-tls"         "EdgionTls leak-cert"
        "EdgionPlugins leak-plugins" "EdgionStreamPlugins leak-stream-plugins"
        "BackendTLSPolicy leak-btls" "Service leak-svc"
        "EndpointSlice leak-svc"
    )
    for entry in "${all_resources[@]}"; do
        local kind name
        kind=$(echo "$entry" | awk '{print $1}')
        name=$(echo "$entry" | awk '{print $2}')
        ctl_delete "$kind" "$name"
    done
    sleep "$CLEANUP_WAIT"
}

# =============================================================================
# Scenarios
# =============================================================================
scenario_orphan_route() {
    log_scenario "Orphan route (non-existent Gateway + Service)"
    ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-orphan.yaml"
    sleep "$SYNC_WAIT"
    ctl_delete "HTTPRoute" "leak-orphan"
    sleep "$CLEANUP_WAIT"
    if verify_all_baseline; then record_result "orphan_route" "pass"
    else record_result "orphan_route" "fail"; fi
}

scenario_wildcard_catchall() {
    log_scenario "Wildcard + catch-all route cleanup"
    ctl_apply "${CONF_DIR}/inject/Service_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/EndpointSlice_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-wildcard.yaml"
    ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-catchall.yaml"
    sleep "$SYNC_WAIT"
    local stats ca wd wd_bl
    stats=$(curl -sf "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
    ca=$(json_field "$stats" ".data.http_routes.has_catch_all")
    wd=$(json_field "$stats" ".data.http_routes.wildcard_domains")
    wd_bl=$(json_field "$BASELINE_STORE_STATS" ".data.http_routes.wildcard_domains")
    log_info "  After inject: catch_all=${ca}, wildcard_domains=${wd} (baseline=${wd_bl})"
    ctl_delete "HTTPRoute" "leak-wildcard"
    ctl_delete "HTTPRoute" "leak-catchall"
    ctl_delete "Service" "leak-svc"
    ctl_delete "EndpointSlice" "leak-svc"
    sleep "$CLEANUP_WAIT"
    if verify_all_baseline; then record_result "wildcard_catchall" "pass"
    else record_result "wildcard_catchall" "fail"; fi
}

scenario_delete_service_before_route() {
    log_scenario "Delete Service before Route"
    ctl_apply "${CONF_DIR}/inject/Service_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/EndpointSlice_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-http.yaml"
    sleep "$SYNC_WAIT"
    ctl_delete "Service" "leak-svc"
    ctl_delete "EndpointSlice" "leak-svc"
    sleep "$SYNC_WAIT"
    ctl_delete "HTTPRoute" "leak-http"
    sleep "$CLEANUP_WAIT"
    if verify_all_baseline; then record_result "delete_svc_before_route" "pass"
    else record_result "delete_svc_before_route" "fail"; fi
}

scenario_delete_route_before_service() {
    log_scenario "Delete Route before Service"
    ctl_apply "${CONF_DIR}/inject/Service_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/EndpointSlice_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-http.yaml"
    sleep "$SYNC_WAIT"
    ctl_delete "HTTPRoute" "leak-http"
    sleep "$SYNC_WAIT"
    ctl_delete "Service" "leak-svc"
    ctl_delete "EndpointSlice" "leak-svc"
    sleep "$CLEANUP_WAIT"
    if verify_all_baseline; then record_result "delete_route_before_svc" "pass"
    else record_result "delete_route_before_svc" "fail"; fi
}

scenario_rapid_fire() {
    log_scenario "Rapid fire inject+delete (no wait)"
    for i in $(seq 1 5); do
        ctl_apply "${CONF_DIR}/inject/Service_leak-svc.yaml"
        ctl_apply "${CONF_DIR}/inject/EndpointSlice_leak-svc.yaml"
        ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-http.yaml"
        ctl_apply "${CONF_DIR}/inject/GRPCRoute_leak-grpc.yaml"
        ctl_apply "${CONF_DIR}/inject/TCPRoute_leak-tcp.yaml"
        ctl_delete "HTTPRoute" "leak-http"
        ctl_delete "GRPCRoute" "leak-grpc"
        ctl_delete "TCPRoute" "leak-tcp"
        ctl_delete "Service" "leak-svc"
        ctl_delete "EndpointSlice" "leak-svc"
    done
    sleep "$CLEANUP_WAIT"
    if verify_all_baseline; then record_result "rapid_fire" "pass"
    else record_result "rapid_fire" "fail"; fi
}

scenario_duplicate_apply() {
    log_scenario "Duplicate apply (same config 5x)"
    ctl_apply "${CONF_DIR}/inject/Service_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/EndpointSlice_leak-svc.yaml"
    for i in $(seq 1 5); do
        ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-http.yaml"
        ctl_apply "${CONF_DIR}/inject/EdgionPlugins_leak-plugins.yaml"
        ctl_apply "${CONF_DIR}/inject/BackendTLSPolicy_leak-btls.yaml"
    done
    sleep "$SYNC_WAIT"
    ctl_delete "HTTPRoute" "leak-http"
    ctl_delete "EdgionPlugins" "leak-plugins"
    ctl_delete "BackendTLSPolicy" "leak-btls"
    ctl_delete "Service" "leak-svc"
    ctl_delete "EndpointSlice" "leak-svc"
    sleep "$CLEANUP_WAIT"
    if verify_all_baseline; then record_result "duplicate_apply" "pass"
    else record_result "duplicate_apply" "fail"; fi
}

scenario_stream_plugin() {
    log_scenario "Stream plugin (EdgionStreamPlugins + TCPRoute with annotation)"
    ctl_apply "${CONF_DIR}/inject/Service_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/EndpointSlice_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/EdgionStreamPlugins_leak-stream.yaml"
    ctl_apply "${CONF_DIR}/inject/TCPRoute_leak-tcp-sp.yaml"
    sleep "$SYNC_WAIT"
    local stats sp sp_bl
    stats=$(curl -sf "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
    sp=$(json_field "$stats" ".data.stream_plugin_store.plugins")
    sp_bl=$(json_field "$BASELINE_STORE_STATS" ".data.stream_plugin_store.plugins")
    log_info "  stream_plugin_store: ${sp} (baseline=${sp_bl})"
    ctl_delete "EdgionStreamPlugins" "leak-stream-plugins"
    ctl_delete "TCPRoute" "leak-tcp-sp"
    ctl_delete "Service" "leak-svc"
    ctl_delete "EndpointSlice" "leak-svc"
    sleep "$CLEANUP_WAIT"
    if verify_all_baseline; then record_result "stream_plugin" "pass"
    else record_result "stream_plugin" "fail"; fi
}

scenario_backend_tls_policy() {
    log_scenario "BackendTLSPolicy lifecycle"
    ctl_apply "${CONF_DIR}/inject/Service_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/EndpointSlice_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/BackendTLSPolicy_leak-btls.yaml"
    sleep "$SYNC_WAIT"
    local stats btls btls_bl
    stats=$(curl -sf "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
    btls=$(json_field "$stats" ".data.backend_tls_policy.policies")
    btls_bl=$(json_field "$BASELINE_STORE_STATS" ".data.backend_tls_policy.policies")
    log_info "  backend_tls_policy.policies: ${btls} (baseline=${btls_bl})"
    ctl_delete "BackendTLSPolicy" "leak-btls"
    ctl_delete "Service" "leak-svc"
    ctl_delete "EndpointSlice" "leak-svc"
    sleep "$CLEANUP_WAIT"
    if verify_all_baseline; then record_result "backend_tls_policy" "pass"
    else record_result "backend_tls_policy" "fail"; fi
}

scenario_mixed_lifecycle() {
    log_scenario "Mixed lifecycle (add while deleting)"
    ctl_apply "${CONF_DIR}/inject/Service_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/EndpointSlice_leak-svc.yaml"
    ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-http.yaml"
    sleep "$SYNC_WAIT"
    ctl_delete "HTTPRoute" "leak-http"
    ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-wildcard.yaml"
    ctl_apply "${CONF_DIR}/inject/HTTPRoute_leak-catchall.yaml"
    ctl_apply "${CONF_DIR}/inject/GRPCRoute_leak-grpc.yaml"
    sleep "$SYNC_WAIT"
    ctl_delete "HTTPRoute" "leak-wildcard"
    ctl_delete "HTTPRoute" "leak-catchall"
    ctl_delete "GRPCRoute" "leak-grpc"
    ctl_delete "Service" "leak-svc"
    ctl_delete "EndpointSlice" "leak-svc"
    sleep "$CLEANUP_WAIT"
    if verify_all_baseline; then record_result "mixed_lifecycle" "pass"
    else record_result "mixed_lifecycle" "fail"; fi
}

scenario_full_blast() {
    log_scenario "Full multi-protocol blast (all resource types)"
    for file in "${CONF_DIR}/inject/"*.yaml; do ctl_apply "$file"; done
    sleep "$SYNC_WAIT"
    local stats hr hr_bl
    stats=$(curl -sf "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
    hr=$(json_field "$stats" ".data.http_routes.http_routes")
    hr_bl=$(json_field "$BASELINE_STORE_STATS" ".data.http_routes.http_routes")
    log_info "  http_routes: ${hr} (baseline=${hr_bl})"
    cleanup_all_test_resources
    if verify_all_baseline; then record_result "full_blast" "pass"
    else record_result "full_blast" "fail"; fi
}

# =============================================================================
# Main
# =============================================================================
main() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --sync-wait)      SYNC_WAIT=$2; shift 2 ;;
            --cleanup-wait)   CLEANUP_WAIT=$2; shift 2 ;;
            --no-start)       DO_START=false; shift ;;
            --keep-alive)     DO_CLEANUP=false; shift ;;
            -h|--help)
                echo "Usage: $0 [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  --sync-wait N      Seconds to wait after inject (default: 2)"
                echo "  --cleanup-wait N   Seconds to wait after delete (default: 5)"
                echo "  --no-start         Attach to already-running environment"
                echo "  --keep-alive       Don't stop services after test"
                exit 0
                ;;
            *) log_error "Unknown option: $1"; exit 1 ;;
        esac
    done

    trap cleanup EXIT

    log_section "Config-Sync Advanced Leak Detection Test"
    echo "  Controller:    ${CONTROLLER_URL}"
    echo "  Gateway Admin: ${GATEWAY_ADMIN_URL}"
    echo ""

    if $DO_START; then
        start_environment
    else
        log_info "Attaching to existing environment"
    fi

    check_prerequisites

    log_section "Loading base configs"
    ctl_apply "${CONF_DIR}/base/Gateway.yaml"
    sleep "$SYNC_WAIT"

    capture_baseline

    log_section "Running advanced scenarios"
    scenario_orphan_route
    scenario_wildcard_catchall
    scenario_delete_service_before_route
    scenario_delete_route_before_service
    scenario_rapid_fire
    scenario_duplicate_apply
    scenario_stream_plugin
    scenario_backend_tls_policy
    scenario_mixed_lifecycle
    scenario_full_blast

    log_section "Cleaning up base configs"
    ctl_delete "Gateway" "leak-test-gateway"

    log_section "Results"
    echo "  Total:   ${TOTAL}"
    echo "  Passed:  ${PASSED}"
    echo "  Failed:  ${FAILED}"
    echo ""

    if [ "$FAILED" -gt 0 ]; then
        log_error "LEAK DETECTED in ${FAILED} scenario(s)"
        exit 1
    else
        log_success "All ${TOTAL} scenarios passed — no config leaks detected"
        exit 0
    fi
}

main "$@"
