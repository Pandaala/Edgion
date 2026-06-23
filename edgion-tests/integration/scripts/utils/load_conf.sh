#!/bin/bash
# =============================================================================
# LoadTestconfig
# directory: Resource/Item ( HTTPRoute/Match)
# =============================================================================

set -e

# 
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# projectdirectory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# Coverage mode: switch to instrumented binary directory.
# LLVM_PROFILE_FILE override is critical: show-env defaults to target/Edgion-*.profraw,
# but cargo llvm-cov report only scans target/llvm-cov-target/*.profraw — keep them aligned.
if [ "${EDGION_COVERAGE:-0}" = "1" ]; then
    eval $(cargo llvm-cov show-env --export-prefix 2>/dev/null)
    export CARGO_TARGET_DIR="${PROJECT_ROOT}/target/llvm-cov-target"
    export LLVM_PROFILE_FILE="${PROJECT_ROOT}/target/llvm-cov-target/Edgion-%p-%10m.profraw"
    BIN_DIR="${PROJECT_ROOT}/target/llvm-cov-target/debug"
else
    BIN_DIR="${PROJECT_ROOT}/target/debug"
fi

# config
CONF_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf"
EDGION_CLI="${BIN_DIR}/edgion-cli"
CONTROLLER_URL="${EDGION_CONTROLLER_URL:-http://127.0.0.1:12101}"
CONTROLLER_PROBE_PORT="${CONTROLLER_PROBE_PORT:-12100}"
CONTROLLER_PROBE_URL="${CONTROLLER_PROBE_URL:-http://127.0.0.1:${CONTROLLER_PROBE_PORT}}"
GATEWAY_ADMIN_URL="${EDGION_GATEWAY_ADMIN_URL:-http://127.0.0.1:12001}"

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

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# Runtime-generated certificate Secrets should never be loaded from conf/.
is_runtime_generated_secret_template() {
    local relative_path=$1
    case "$relative_path" in
        "base/Secret_edgion-test_edge-tls.yaml")
            return 0
            ;;
        "EdgionTls/mTLS/Secret_edge_client-ca.yaml")
            return 0
            ;;
        "EdgionTls/mTLS/Secret_edge_ca-chain.yaml")
            return 0
            ;;
        "EdgionTls/mTLS/Secret_edge_mtls-server.yaml")
            return 0
            ;;
        "HTTPRoute/Backend/BackendTLS/Secret_backend-ca.yaml")
            return 0
            ;;
        "HTTPRoute/Backend/BackendTLS/ClientCert_edge_backend-client-cert.yaml")
            return 0
            ;;
        "Gateway/TLS/BackendTLSGatewayCert/Secret_edge_gateway-client-cert.yaml")
            return 0
            ;;
        "EdgionPlugins/HeaderCertAuth/01_Secret_default_header-cert-ca.yaml")
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

# =============================================================================
# helpinfo
# =============================================================================
show_help() {
    echo "LoadTestconfig（directory）"
    echo ""
    echo ": $0 [OPTIONS] <SUITE>"
    echo ""
    echo "SUITE (path):"
    echo "  base                    config (GatewayClass, EdgionGatewayConfig)"
    echo "  HTTPRoute               HTTPRoute allTestconfig"
    echo "  HTTPRoute/Basic         HTTPRoute Test"
    echo "  HTTPRoute/Match         HTTPRoute Test"
    echo "  HTTPRoute/Backend       HTTPRoute afterTest ( LBPolicy, WeightedBackend, Timeout)"
    echo "  HTTPRoute/Filters       HTTPRoute Test ( Redirect, Security)"
    echo "  HTTPRoute/Protocol      HTTPRoute Test ( WebSocket)"
    echo "  grpc                    gRPC Testconfig"
    echo "  tcp                     TCP Testconfig"
    echo "  udp                     UDP Testconfig"
    echo "  all                     Loadallconfig"
    echo ""
    echo "OPTIONS:"
    echo "  --verify     Loadafterverifyresourcesync"
    echo "  --wait N     Wait N configtake effect (default: 2)"
    echo "  -h, --help   Showhelp"
    echo ""
    echo ":"
    echo "  $0 base                      # Loadconfig"
    echo "  $0 HTTPRoute/Match           # Load HTTPRoute Testconfig"
    echo "  $0 HTTPRoute/Backend         # Load HTTPRoute afterTestconfig"
    echo "  $0 --verify HTTPRoute/Basic  # Loadverify HTTPRoute config"
}

# =============================================================================
# Checkservicestatus
# =============================================================================
check_services() {
    log_info "Checkservicestatus..."
    
    # Check controller health (liveness)
    if ! curl -sf "${CONTROLLER_PROBE_URL}/health" > /dev/null 2>&1; then
        log_error "Controller Run (${CONTROLLER_PROBE_URL})"
        return 1
    fi
    
    log_success "Controller Run"
    return 0
}

# =============================================================================
# Wait for controller to be ready (ConfigServer initialized)
# =============================================================================
wait_for_ready() {
    local max_attempts=${1:-30}
    local attempt=0
    
    log_info " Controller ConfigServer ..."
    
    while [ $attempt -lt $max_attempts ]; do
        if curl -sf "${CONTROLLER_PROBE_URL}/ready" > /dev/null 2>&1; then
            log_success "Controller ConfigServer "
            return 0
        fi
        
        attempt=$((attempt + 1))
        if [ $attempt -lt $max_attempts ]; then
            sleep 1
        fi
    done
    
    log_error "Controller ConfigServer  ( ${max_attempts}s)"
    return 1
}

# =============================================================================
# use edgion-cli Loadfile
# 
# FileSystemWriter  Kind_namespace_name.yaml ，
# 
# =============================================================================
apply_file() {
    local file=$1
    local filename=$(basename "$file")
    
    #  edgion-cli apply
    log_info "Load $filename via API..."
    if "$EDGION_CLI" --server "$CONTROLLER_URL" apply -f "$file" 2>&1; then
        log_success "$filename Loadsuccess"
        return 0
    else
        log_error "$filename Loadfailed"
        return 1
    fi
}

# =============================================================================
# Loaddirectoryall yaml file
# =============================================================================
load_directory_recursive() {
    local dir=$1
    local suite_name=$2
    local failed=false
    local count=0
    
    if [ ! -d "$dir" ]; then
        log_warn "directory: $dir"
        return 1
    fi
    
    # Skip dynamic delta dirs (updates / updates_grouped / delete); only initial / initial_grouped are pre-loaded.
    if [[ "$dir" =~ /DynamicTest/updates ]] || [[ "$dir" =~ /DynamicTest/delete ]]; then
        log_info "Skipping dynamic update dir: $dir"
        return 0
    fi

    # Walk YAML files, excluding any updates*/ and delete*/ subdirs under DynamicTest.
    local files=$(find "$dir" -type f \( -name "*.yaml" -o -name "*.yml" \) \
        -not -path "*/DynamicTest/updates/*" \
        -not -path "*/DynamicTest/updates_grouped/*" \
        -not -path "*/DynamicTest/delete/*" | sort)
    
    if [ -z "$files" ]; then
        log_warn "$suite_name: configfile"
        return 0
    fi
    
    for file in $files; do
        local relative_file="${file#${CONF_DIR}/}"
        if is_runtime_generated_secret_template "$relative_file"; then
            log_info "Skipping runtime-managed Secret template: $relative_file"
            continue
        fi

        if ! apply_file "$file"; then
            failed=true
        fi
        count=$((count + 1))
    done
    
    if $failed; then
        log_error "$suite_name: partialconfigLoadfailed"
        return 1
    else
        log_success "$suite_name: $count configLoadcompleted"
        return 0
    fi
}

# =============================================================================
# verifyresourcesync
# =============================================================================
verify_sync() {
    log_section "verifyresourcesync"
    
    if [ ! -f "${BIN_DIR}/resource_diff" ]; then
        log_warn "resource_diff Build，Skipverify"
        return 0
    fi
    
    "${BIN_DIR}/resource_diff" \
        --controller-url "$CONTROLLER_URL" \
        --gateway-url "$GATEWAY_ADMIN_URL" \
        --gateway-probe-url "${EDGION_GATEWAY_PROBE_URL:-http://127.0.0.1:12000}"
    
    return $?
}

# =============================================================================
# allLoad suite 
# =============================================================================
get_all_suites() {
    local suites="base"
    
    # HTTPRoute alldirectory
    for subdir in "${CONF_DIR}/HTTPRoute"/*; do
        if [ -d "$subdir" ]; then
            local name=$(basename "$subdir")
            suites="$suites HTTPRoute/$name"
        fi
    done
    
    # resource
    for resource in grpc grpc-match tcp udp mtls security real-ip backend-tls_tcp plugins; do
        if [ -d "${CONF_DIR}/${resource}" ]; then
            suites="$suites $resource"
        fi
    done

    # Status test suites
    for subdir in "${CONF_DIR}/Status"/*; do
        if [ -d "$subdir" ]; then
            local name=$(basename "$subdir")
            suites="$suites Status/$name"
        fi
    done
    
    echo "$suites"
}

# =============================================================================
# 
# =============================================================================
main() {
    local suites=""
    local do_verify=false
    local wait_time=2
    
    # Parseargs
    while [[ $# -gt 0 ]]; do
        case $1 in
            --verify)
                do_verify=true
                shift
                ;;
            --wait)
                wait_time=$2
                shift 2
                ;;
            -h|--help)
                show_help
                exit 0
                ;;
            -*)
                log_error "unknownoptions: $1"
                show_help
                exit 1
                ;;
            *)
                suites="$suites $1"
                shift
                ;;
        esac
    done
    
    suites=$(echo "$suites" | xargs)
    
    if [ -z "$suites" ]; then
        log_error "PleasespecifyLoadconfig suite"
        show_help
        exit 1
    fi
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}LoadTestconfig${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Controller: ${CONTROLLER_URL}"
    echo -e "Suite:      ${suites}"
    echo ""
    
    # Check edgion-cli
    if [ ! -f "$EDGION_CLI" ]; then
        log_error "edgion-cli Build，PleaseRun prepare.sh"
        exit 1
    fi
    
    # Checkservice
    if ! check_services; then
        exit 1
    fi
    
    # Wait for ConfigServer to be ready before loading configs
    if ! wait_for_ready 30; then
        exit 1
    fi

    # Auth is handled automatically by edgion-cli via ensure_auth() on each command.
    # It reads local_auth credentials from config/edgion-controller.yaml when present.

    #  "all"
    if [ "$suites" = "all" ]; then
        suites=$(get_all_suites)
        log_info "Loadallconfig: $suites"
    fi
    
    local failed=false
    
    # Load suite
    for suite in $suites; do
        log_section "Load $suite config"
        
        local suite_dir="${CONF_DIR}/${suite}"
        
        if ! load_directory_recursive "$suite_dir" "$suite"; then
            failed=true
        fi
    done
    
    # Waitconfigtake effect
    if [ $wait_time -gt 0 ]; then
        log_info "Wait ${wait_time}s configtake effect..."
        sleep $wait_time
    fi
    
    # verify
    if $do_verify; then
        if ! verify_sync; then
            failed=true
        fi
    fi
    
    # 
    log_section "completed"
    
    if $failed; then
        log_error "partialconfigLoadfailed"
        exit 1
    else
        log_success "allconfigLoadsuccess"
        exit 0
    fi
}

main "$@"
