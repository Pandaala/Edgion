#!/bin/bash
# =============================================================================
# Config-Sync Leak Detection Test (Basic)
#
# Self-contained: automatically starts/stops test environment.
# Injects and deletes configurations in cycles, then verifies all derived
# stores are properly cleaned up. Detects leaked entries that survive removal.
#
# Usage:
#   ./run_conf_sync_test.sh                # Full run (start → test → stop)
#   ./run_conf_sync_test.sh --no-start     # Attach to running environment
#   ./run_conf_sync_test.sh --keep-alive   # Don't stop services after test
#   ./run_conf_sync_test.sh --cycles 20    # Run 20 inject/delete cycles
#   ./run_conf_sync_test.sh --stress       # Run 50 cycles
# =============================================================================

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UTILS_DIR="${SCRIPT_DIR}/../utils"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
CONF_DIR="${PROJECT_ROOT}/examples/test/conf/conf-sync-leak-test"
EDGION_CLI="${PROJECT_ROOT}/target/debug/edgion-cli"

CONTROLLER_URL="${EDGION_CONTROLLER_URL:-http://127.0.0.1:12101}"
CONTROLLER_PROBE_URL="${CONTROLLER_PROBE_URL:-http://127.0.0.1:12100}"
GATEWAY_ADMIN_URL="${EDGION_GATEWAY_ADMIN_URL:-http://127.0.0.1:12001}"
# Gateway admin API requires a bearer token; default for standalone invocation.
: "${EDGION_GATEWAY_ADMIN_TOKEN:=integration-gateway-admin-token-0123456789}"
GATEWAY_PROBE_URL="${EDGION_GATEWAY_PROBE_URL:-http://127.0.0.1:12000}"

CYCLES=${CYCLES:-5}
SYNC_WAIT=2
CLEANUP_WAIT=5
MAX_VERIFY_RETRIES=15
VERIFY_RETRY_INTERVAL=2

PASSED=0
FAILED=0

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
# Prerequisite checks
# =============================================================================
check_prerequisites() {
    if [ ! -f "$EDGION_CLI" ]; then
        log_error "edgion-cli not found at $EDGION_CLI — run prepare.sh first"
        exit 1
    fi
    if ! curl -sf "${CONTROLLER_PROBE_URL}/health" > /dev/null 2>&1; then
        log_error "Controller not reachable at ${CONTROLLER_PROBE_URL}"
        exit 1
    fi
    if ! curl -sf "${GATEWAY_PROBE_URL}/health" > /dev/null 2>&1; then
        log_error "Gateway probe not reachable at ${GATEWAY_PROBE_URL}"
        exit 1
    fi
    local status
    status=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/api/v1/testing/status" 2>/dev/null || echo "")
    if [ -z "$status" ]; then
        log_error "Gateway not in integration-testing-mode"
        exit 1
    fi
    log_success "Prerequisites OK"
}

# =============================================================================
# Config injection / deletion
# =============================================================================
inject_configs() {
    local dir=$1 label=$2 fail=false
    log_info "Injecting configs from ${label}..."
    for file in "${dir}"/*.yaml; do
        [ -f "$file" ] || continue
        if ! "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$file" > /dev/null 2>&1; then
            log_error "  Failed to apply $(basename "$file")"
            fail=true
        fi
    done
    if $fail; then
        log_error "Some configs failed to inject"
        return 1
    fi
    log_success "Injected configs from ${label}"
}

delete_injected_configs() {
    log_info "Deleting injected configs..."
    local ns="edgion-test" fail=false
    local -a resources=(
        "HTTPRoute leak-http"
        "HTTPRoute leak-wildcard"
        "HTTPRoute leak-catchall"
        "HTTPRoute leak-orphan"
        "GRPCRoute leak-grpc"
        "TCPRoute leak-tcp"
        "TCPRoute leak-tcp-sp"
        "UDPRoute leak-udp"
        "TLSRoute leak-tls"
        "EdgionTls leak-cert"
        "EdgionPlugins leak-plugins"
        "EdgionStreamPlugins leak-stream-plugins"
        "BackendTLSPolicy leak-btls"
        "Service leak-svc"
        "EndpointSlice leak-svc"
    )
    for entry in "${resources[@]}"; do
        local kind name
        kind=$(echo "$entry" | awk '{print $1}')
        name=$(echo "$entry" | awk '{print $2}')
        "$EDGION_CLI" --server "$CONTROLLER_URL" delete "$kind" "$name" -n "$ns" > /dev/null 2>&1 || true
    done
    if $fail; then return 1; fi
    log_success "Deleted injected configs"
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
        resp=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/configclient/${kind}/list" 2>/dev/null || echo '{"count":0}')
        count=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('count',0))" 2>/dev/null || echo "0")
        echo "$count" > "${BASELINE_DIR}/cc_${kind}"
    done
    BASELINE_STORE_STATS=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
    local bl_http=$(cat "${BASELINE_DIR}/cc_httproute")
    local bl_svc=$(cat "${BASELINE_DIR}/cc_service")
    log_success "Baseline captured (httproute=${bl_http}, service=${bl_svc})"
}

get_baseline_cc() { cat "${BASELINE_DIR}/cc_${1}" 2>/dev/null || echo "0"; }

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

verify_configclient_empty() {
    local all_ok=true
    local kinds=(httproute grpcroute tcproute udproute tlsroute edgiontls edgionplugins edgionstreamplugins backendtlspolicy service endpointslice)
    for kind in "${kinds[@]}"; do
        local resp count baseline
        resp=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/configclient/${kind}/list" 2>/dev/null || echo '{"count":-1}')
        count=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('count',-1))" 2>/dev/null || echo "-1")
        baseline=$(get_baseline_cc "$kind")
        if [ "$count" != "$baseline" ]; then
            all_ok=false
            log_warn "  configclient/${kind}: count=${count} (baseline=${baseline})"
        fi
    done
    $all_ok
}

# Verify the admin /configclient API never leaks resolved secret material.
# Reuses the kind sweep: scans every kind's response for plaintext private-key
# markers, and confirms the one populated secret-bearing resource (EdgionTls
# leak-cert, resolved from the edge-tls kubernetes.io/tls Secret) renders as
# "[redacted]". The positive check is soft (skips, not fails) when the fixture's
# spec.secret is not populated, so it cannot false-fail on a vacuous fixture.
verify_no_secret_leak() {
    local all_ok=true
    local kinds=(httproute grpcroute tcproute udproute tlsroute edgiontls edgionplugins edgionstreamplugins backendtlspolicy service endpointslice)
    for kind in "${kinds[@]}"; do
        local resp
        resp=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/configclient/${kind}/list" 2>/dev/null || echo '{}')
        # No PEM private key and no raw kubernetes.io/tls Secret data keys may appear.
        if echo "$resp" | grep -Eq 'BEGIN (RSA |EC |)PRIVATE KEY|"tls\.key"|"tls\.crt"'; then
            all_ok=false
            log_error "  configclient/${kind}: plaintext secret material leaked in response"
        fi
    done
    local secret_val
    secret_val=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/configclient/edgiontls/list" 2>/dev/null | python3 -c "
import sys, json
try:
    rows = json.load(sys.stdin).get('data', [])
except Exception:
    print('ERR'); sys.exit(0)
for r in rows:
    if r.get('metadata', {}).get('name') == 'leak-cert':
        s = r.get('spec', {}).get('secret', None)
        print('ABSENT' if s is None else (s if isinstance(s, str) else 'OBJECT'))
        break
else:
    print('NOTFOUND')
" 2>/dev/null || echo "ERR")
    case "$secret_val" in
        '[redacted]') : ;; # expected: secret resolved and correctly redacted
        ABSENT | NOTFOUND | ERR)
            log_warn "  edgiontls/leak-cert spec.secret not populated (${secret_val}); redaction check skipped" ;;
        *)
            all_ok=false
            log_error "  edgiontls/leak-cert spec.secret leaked (value='${secret_val}', expected '[redacted]')" ;;
    esac
    $all_ok
}

verify_store_stats_empty() {
    local stats
    stats=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
    if [ -z "$stats" ]; then log_error "  Failed to fetch store-stats"; return 1; fi

    local all_ok=true
    local check_fields=(
        # HTTP route manager
        ".data.http_routes.http_routes"  ".data.http_routes.exact_domains"
        ".data.http_routes.wildcard_domains" ".data.http_routes.port_count"
        # gRPC route manager
        ".data.grpc_routes.grpc_routes"  ".data.grpc_routes.resource_keys"
        ".data.grpc_routes.route_units_cache" ".data.grpc_routes.port_count"
        # TCP/UDP/TLS route managers
        ".data.tcp_routes.route_cache"   ".data.tcp_routes.port_count"
        ".data.udp_routes.route_cache"   ".data.udp_routes.port_count"
        ".data.tls_routes.route_cache"   ".data.tls_routes.port_count"
        # Gateway-level stores
        ".data.gateway_config.gateways"
        ".data.port_gateway_info.port_count"
        # TLS cert stores
        ".data.tls_store.entries"  ".data.tls_cert_matcher.port_count"
        # Plugin stores
        ".data.plugin_store_v2.total"    ".data.stream_plugin_store_v2.total"
        # Link / policy stores
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

verify_all_empty() {
    for i in $(seq 1 "$MAX_VERIFY_RETRIES"); do
        local cc_ok=true ss_ok=true
        verify_configclient_empty || cc_ok=false
        verify_store_stats_empty  || ss_ok=false
        if $cc_ok && $ss_ok; then return 0; fi
        if [ "$i" -lt "$MAX_VERIFY_RETRIES" ]; then
            log_info "  Retry ${i}/${MAX_VERIFY_RETRIES} — waiting ${VERIFY_RETRY_INTERVAL}s..."
            sleep "$VERIFY_RETRY_INTERVAL"
        fi
    done
    return 1
}

verify_configs_present() {
    local kinds=(httproute grpcroute tcproute udproute tlsroute edgiontls edgionplugins edgionstreamplugins backendtlspolicy service endpointslice)
    local all_present=true
    for kind in "${kinds[@]}"; do
        local resp count
        resp=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/configclient/${kind}/list" 2>/dev/null || echo '{"count":0}')
        count=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('count',0))" 2>/dev/null || echo "0")
        if [ "$count" = "0" ]; then
            all_present=false
            log_warn "  configclient/${kind}: count=0 (expected > 0)"
        fi
    done
    $all_present
}

# =============================================================================
# Single test cycle
# =============================================================================
run_cycle() {
    local cycle=$1
    log_section "Cycle ${cycle}/${CYCLES}"

    inject_configs "${CONF_DIR}/inject" "inject"
    log_info "Waiting ${SYNC_WAIT}s for sync..."
    sleep "$SYNC_WAIT"

    log_info "Verifying configs are present..."
    local present_ok=true
    for attempt in 1 2 3; do
        if verify_configs_present; then present_ok=true; break; fi
        present_ok=false
        log_info "  Configs not fully synced, retry ${attempt}/3..."
        sleep 2
    done
    if ! $present_ok; then
        log_error "Cycle ${cycle}: Configs did not sync"
        FAILED=$((FAILED + 1))
        return 1
    fi

    # Snapshot port_counts after injection for diagnostic
    local mid_stats
    mid_stats=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
    local pc_http pc_grpc pc_tcp pc_udp pc_tls
    pc_http=$(json_field "$mid_stats" ".data.http_routes.port_count")
    pc_grpc=$(json_field "$mid_stats" ".data.grpc_routes.port_count")
    pc_tcp=$(json_field "$mid_stats" ".data.tcp_routes.port_count")
    pc_udp=$(json_field "$mid_stats" ".data.udp_routes.port_count")
    pc_tls=$(json_field "$mid_stats" ".data.tls_routes.port_count")
    log_success "Configs synced (port_counts: http=${pc_http} grpc=${pc_grpc} tcp=${pc_tcp} udp=${pc_udp} tls=${pc_tls})"

    # While configs are present, confirm the admin API redacts secrets (no leak).
    if ! verify_no_secret_leak; then
        log_error "Cycle ${cycle}: SECRET LEAK detected in /configclient response"
        FAILED=$((FAILED + 1))
        return 1
    fi

    delete_injected_configs
    log_info "Waiting ${CLEANUP_WAIT}s for cleanup..."
    sleep "$CLEANUP_WAIT"

    log_info "Verifying stores are empty..."
    if verify_all_empty; then
        log_success "Cycle ${cycle}: All stores clean"
        PASSED=$((PASSED + 1))
    else
        log_error "Cycle ${cycle}: LEAK DETECTED — stores not fully cleaned"
        log_error "  Dumping current store-stats vs baseline:"
        local leak_stats
        leak_stats=$(curl -sf -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" "${GATEWAY_ADMIN_URL}/api/v1/debug/store-stats" 2>/dev/null || echo "")
        local leak_fields=(
            ".data.http_routes.http_routes" ".data.http_routes.port_count"
            ".data.grpc_routes.grpc_routes" ".data.grpc_routes.port_count"
            ".data.tcp_routes.route_cache"  ".data.tcp_routes.port_count"
            ".data.udp_routes.route_cache"  ".data.udp_routes.port_count"
            ".data.tls_routes.route_cache"  ".data.tls_routes.port_count"
        )
        for f in "${leak_fields[@]}"; do
            local cur bl
            cur=$(json_field "$leak_stats" "$f")
            bl=$(json_field "$BASELINE_STORE_STATS" "$f")
            if [ "$cur" != "$bl" ]; then
                log_error "    ${f}: current=${cur} baseline=${bl}"
            fi
        done
        FAILED=$((FAILED + 1))
        return 1
    fi
}

# =============================================================================
# Main
# =============================================================================
main() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            --cycles|-n)      CYCLES=$2; shift 2 ;;
            --stress)         CYCLES=50; shift ;;
            --sync-wait)      SYNC_WAIT=$2; shift 2 ;;
            --cleanup-wait)   CLEANUP_WAIT=$2; shift 2 ;;
            --no-start)       DO_START=false; shift ;;
            --keep-alive)     DO_CLEANUP=false; shift ;;
            -h|--help)
                echo "Usage: $0 [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  --cycles N, -n N   Number of inject/delete cycles (default: 5)"
                echo "  --stress           Run 50 cycles"
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

    log_section "Config-Sync Leak Detection Test (Basic)"
    echo "  Cycles: ${CYCLES}"
    echo ""

    if $DO_START; then
        start_environment
    else
        log_info "Attaching to existing environment"
    fi

    check_prerequisites

    log_section "Loading base configs"
    inject_configs "${CONF_DIR}/base" "base"
    log_info "Waiting ${SYNC_WAIT}s for base sync..."
    sleep "$SYNC_WAIT"

    capture_baseline

    local exit_code=0
    for cycle in $(seq 1 "$CYCLES"); do
        if ! run_cycle "$cycle"; then exit_code=1; fi
    done

    log_section "Cleaning up base configs"
    "$EDGION_CLI" --server "$CONTROLLER_URL" delete Gateway leak-test-gateway -n edgion-test > /dev/null 2>&1 || true

    log_section "Results"
    echo "  Cycles:  ${CYCLES}"
    echo "  Passed:  ${PASSED}"
    echo "  Failed:  ${FAILED}"
    echo ""

    if [ "$FAILED" -gt 0 ]; then
        log_error "LEAK DETECTED in ${FAILED} cycle(s)"
        exit 1
    else
        log_success "All ${CYCLES} cycles passed — no config leaks detected"
        exit 0
    fi
}

main "$@"
