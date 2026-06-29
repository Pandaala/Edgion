#!/bin/bash
# =============================================================================
# Startall Edgion TestserviceLoadconfig
# Start: test_server -> controller ->  ->  -> gateway -> verify
# 
#  Admin API (edgion-cli apply) ，FileSystemWriter
# Kind_namespace_name.yaml ，
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
CERTS_DIR="${SCRIPT_DIR}/../gen_certs"
source "${SCRIPT_DIR}/inject_conf_sync_mtls.sh"

# Coverage mode: switch to instrumented binary directory and export coverage env to children.
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

# Workdirectory
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/testing_${TIMESTAMP}"
GENERATED_SECRET_DIR="${WORK_DIR}/generated-secrets"

#  WORK_DIR scriptuse
export EDGION_WORK_DIR="$WORK_DIR"
export EDGION_GENERATED_SECRET_DIR="$GENERATED_SECRET_DIR"

# directory
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"

# configfile
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.yaml"
GATEWAY_CONFIG="${PROJECT_ROOT}/config/edgion-gateway.yaml"

# Admin API token (token-only model). The Controller authenticates the Admin API
# with a single pre-shared bearer token read from EDGION_ADMIN_TOKEN; it no longer
# uses username/password. Honor an inherited value (e.g. exported by
# run_integration.sh — the single source of truth) or fall back to the same fixed
# test token so this script also works standalone. The Controller started below
# registers it as the allow-all admin token, and edgion-cli (config load) reads the
# same value via EDGION_TOKEN.
export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
export EDGION_TOKEN="${EDGION_ADMIN_TOKEN}"

# serviceport
TEST_SERVER_HTTP_PORT=30001
CONTROLLER_ADMIN_PORT=12101
CONTROLLER_PROBE_PORT="${CONTROLLER_PROBE_PORT:-12100}"
CONTROLLER_GRPC_PORT="${EDGION_TEST_CONTROLLER_GRPC_PORT:-12151}"
# Gateway portuse http Testsuiteport（31000）
GATEWAY_HTTP_PORT=31000
GATEWAY_ADMIN_PORT="${EDGION_TEST_GATEWAY_ADMIN_PORT:-12001}"
export EDGION_GATEWAY_ADMIN_URL="http://127.0.0.1:${GATEWAY_ADMIN_PORT}"
# Gateway admin API now requires a bearer token. Export a fixed test token so the
# gateway process reads it from env and the test scripts can reuse it on every
# admin-port call. Override via EDGION_GATEWAY_ADMIN_TOKEN if already set.
export EDGION_GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-integration-gateway-admin-token-0123456789}"
GATEWAY_PROBE_PORT="${GATEWAY_PROBE_PORT:-12000}"

# LoadTestsuite（default，Loadall）
SUITES=""

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

log_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# =============================================================================
# Showhelp
# =============================================================================
show_help() {
    echo "Start Edgion TestserviceLoadconfig"
    echo ""
    echo ": $0 [OPTIONS]"
    echo ""
    echo "OPTIONS:"
    echo "  --suites <list>    specifyLoadTestsuite（comma separated）"
    echo "  --controller-grpc-port <port>  override controller/gateway gRPC sync port"
    echo "                     default：Loadall (http,grpc,tcp,udp,http-match,...)"
    echo "  -h, --help         Showhelp"
    echo ""
    echo ":"
    echo "  $0                          # Loadallconfig"
    echo "  $0 --suites http,https      # OnlyLoad http  https config"
}

# =============================================================================
# Cleanupallprocess
# =============================================================================
kill_all_processes() {
    log_section "Cleanupallprocess"
    
    #  kill allprocess
    pkill -9 -f "edgion-controller" 2>/dev/null && log_info "alreadyStop edgion-controller" || true
    pkill -9 -f "edgion-gateway" 2>/dev/null && log_info "alreadyStop edgion-gateway" || true
    pkill -9 -f "test_server" 2>/dev/null && log_info "alreadyStop test_server" || true
    
    # portrelease
    sleep 2
    
    # verifyportalreadyrelease
    local ports_busy=false
    if nc -z 127.0.0.1 $TEST_SERVER_HTTP_PORT 2>/dev/null; then
        log_error "port $TEST_SERVER_HTTP_PORT occupied"
        ports_busy=true
    fi
    if nc -z 127.0.0.1 $CONTROLLER_ADMIN_PORT 2>/dev/null; then
        log_error "port $CONTROLLER_ADMIN_PORT occupied"
        ports_busy=true
    fi
    if nc -z 127.0.0.1 $CONTROLLER_GRPC_PORT 2>/dev/null; then
        log_error "port $CONTROLLER_GRPC_PORT occupied"
        ports_busy=true
    fi
    if nc -z 127.0.0.1 $GATEWAY_HTTP_PORT 2>/dev/null; then
        log_error "port $GATEWAY_HTTP_PORT occupied"
        ports_busy=true
    fi
    
    if $ports_busy; then
        log_error "releaseport，PleaseCheck"
        exit 1
    fi
    
    log_success "allprocessalreadyCleanup，portalreadyrelease"
}

# =============================================================================
# Checkbinaryfile
# =============================================================================
check_binaries() {
    log_section "Checkbinaryfile"
    
    local missing=false
    
    if [ ! -f "${BIN_DIR}/edgion-controller" ]; then
        log_error "edgion-controller Build"
        missing=true
    fi
    
    if [ ! -f "${BIN_DIR}/edgion-gateway" ]; then
        log_error "edgion-gateway Build"
        missing=true
    fi
    
    if [ ! -f "${BIN_DIR}/test_server" ]; then
        log_error "test_server Build"
        missing=true
    fi
    
    if [ ! -f "${BIN_DIR}/resource_diff" ]; then
        log_error "resource_diff Build"
        missing=true
    fi

    if [ ! -f "${BIN_DIR}/edgion-cli" ]; then
        log_error "edgion-cli Build"
        missing=true
    fi

    if $missing; then
        log_error "PleaseRun prepare.sh Build"
        exit 1
    fi
    
    log_success "allbinaryfileready"
}

# =============================================================================
# Waitport
# =============================================================================
wait_for_port() {
    local port=$1
    local service_name=$2
    local pid=$3
    local timeout=${4:-30}
    local elapsed=0
    
    log_info "Wait $service_name (port $port)..."
    
    while [ $elapsed -lt $timeout ]; do
        # Checkprocess
        if ! kill -0 $pid 2>/dev/null; then
            log_error "$service_name processalready (PID: $pid)"
            return 1
        fi
        
        # Checkport
        if nc -z 127.0.0.1 $port 2>/dev/null; then
            log_success "$service_name portready (port $port)"
            return 0
        fi
        
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name  ${timeout}s Start"
    return 1
}

# =============================================================================
# Wait HTTP healthCheck (liveness)
# =============================================================================
wait_for_health() {
    local url=$1
    local service_name=$2
    local timeout=${3:-10}
    local elapsed=0
    
    log_info "Check $service_name healthstatus..."
    
    while [ $elapsed -lt $timeout ]; do
        local response=$(curl -sf "$url" 2>/dev/null)
        if [ -n "$response" ]; then
            log_success "$service_name healthCheckpassed"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name healthCheckfailed"
    return 1
}

# =============================================================================
# Wait for readiness check (ConfigServer ready)
# =============================================================================
wait_for_ready() {
    local url=$1
    local service_name=$2
    local timeout=${3:-30}
    local elapsed=0
    
    log_info " $service_name ConfigServer ..."
    
    while [ $elapsed -lt $timeout ]; do
        if curl -sf "$url" >/dev/null 2>&1; then
            log_success "$service_name ConfigServer "
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    
    log_error "$service_name ConfigServer  ( ${timeout}s)"
    return 1
}

check_port_conflict() {
    local port=$1
    local label=$2

    if command -v lsof >/dev/null 2>&1; then
        local port_usage
        # Only treat an active listener as a port conflict. macOS can show
        # stale CLOSED connections for a port that is not actually bound.
        port_usage=$(lsof -n -P -iTCP:"${port}" -sTCP:LISTEN 2>/dev/null || true)
        if [ -n "$port_usage" ]; then
            log_error "$label port $port occupied"
            echo "$port_usage"
            return 1
        fi
    elif nc -z 127.0.0.1 "$port" 2>/dev/null; then
        log_error "$label port $port occupied"
        return 1
    fi

    return 0
}

# =============================================================================
# Start test_server
# =============================================================================
start_test_server() {
    log_section "Start test_server"
    
    # Backend TLS certificatepath
    BACKEND_CERT="${PROJECT_ROOT}/edgion-tests/certs/backend/server.crt"
    BACKEND_KEY="${PROJECT_ROOT}/edgion-tests/certs/backend/server.key"
    BACKEND_MTLS_CLIENT_CA="${PROJECT_ROOT}/edgion-tests/certs/mtls/client-ca.crt"
    BACKEND_CERT_A="${PROJECT_ROOT}/edgion-tests/certs/backend/server-a.crt"
    BACKEND_KEY_A="${PROJECT_ROOT}/edgion-tests/certs/backend/server-a.key"
    BACKEND_CERT_B="${PROJECT_ROOT}/edgion-tests/certs/backend/server-b.crt"
    BACKEND_KEY_B="${PROJECT_ROOT}/edgion-tests/certs/backend/server-b.key"

    # Checkafter TLS certificate
    local https_backend_args=""
    if [ -f "$BACKEND_CERT" ] && [ -f "$BACKEND_KEY" ]; then
        https_backend_args="--https-backend-port 30051 --cert-file $BACKEND_CERT --key-file $BACKEND_KEY"
        log_info " HTTPS afterport 30051"
    else
        log_warning "Backend TLS certificate，Skip HTTPS after"
    fi

    # ext_proc TLS server (reuses the backend server cert; SAN covers 127.0.0.1).
    # Drives the ExtProc-over-TLS scenario (the gateway's rustls ext_proc connector).
    local ext_proc_tls_args=""
    if [ -f "$BACKEND_CERT" ] && [ -f "$BACKEND_KEY" ]; then
        ext_proc_tls_args="--ext-proc-tls-port 30042 --ext-proc-tls-cert $BACKEND_CERT --ext-proc-tls-key $BACKEND_KEY"
        log_info " ext_proc TLS port 30042"
    else
        log_warning "Backend TLS certificate missing, skipping ext_proc TLS server"
    fi

    local https_backend_mtls_args=""
    if [ -f "$BACKEND_CERT" ] && [ -f "$BACKEND_KEY" ] && [ -f "$BACKEND_MTLS_CLIENT_CA" ]; then
        https_backend_mtls_args="--https-backend-mtls-port 30052 --client-ca-file $BACKEND_MTLS_CLIENT_CA"
        log_info " HTTPS mTLS afterport 30052"
    else
        log_warning "Backend mTLS ，Skip HTTPS mTLS after"
    fi

    local https_backend_multi_sni_args=""
    if [ -f "$BACKEND_CERT_A" ] && [ -f "$BACKEND_KEY_A" ] && [ -f "$BACKEND_CERT_B" ] && [ -f "$BACKEND_KEY_B" ]; then
        https_backend_multi_sni_args="--https-backend-multi-sni-port 30026 --multi-sni-cert-a $BACKEND_CERT_A --multi-sni-key-a $BACKEND_KEY_A --multi-sni-cert-b $BACKEND_CERT_B --multi-sni-key-b $BACKEND_KEY_B"
        log_info "HTTPS multi-SNI backend on port 30026"
    else
        log_warning "Backend multi-SNI certs not found, skipping multi-SNI backend (port 30026)"
    fi

    "${BIN_DIR}/test_server" \
        --http-ports "30001,30002,30003" \
        --grpc-ports "30021,30022,30023" \
        --websocket-port 30005 \
        --tcp-port 30010 \
        --udp-port 30011 \
        --tcp-pp2-port 30012 \
        --auth-port 30040 \
        --ext-proc-port 30041 \
        --h2-trailer-malformed-port 30025 \
        --log-level info \
        $ext_proc_tls_args \
        $https_backend_args \
        $https_backend_mtls_args \
        $https_backend_multi_sni_args \
        > "${LOG_DIR}/test_server.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/test_server.pid"
    
    # Waitport
    if ! wait_for_port $TEST_SERVER_HTTP_PORT "test_server" $pid 30; then
        log_error "test_server Startfailed，viewlog: ${LOG_DIR}/test_server.log"
        tail -20 "${LOG_DIR}/test_server.log" 2>/dev/null || true
        exit 1
    fi
    
    # healthCheck
    if ! wait_for_health "http://127.0.0.1:${TEST_SERVER_HTTP_PORT}/health" "test_server" 10; then
        log_error "test_server healthCheckfailed"
        exit 1
    fi
    
    log_success "test_server Startsuccess (PID: $pid)"
}

# =============================================================================
# Start controller
# =============================================================================
start_controller() {
    log_section "Start edgion-controller"

    if ! check_port_conflict "$CONTROLLER_GRPC_PORT" "edgion-controller gRPC"; then
        log_error "Please release controller gRPC port before starting tests"
        exit 1
    fi
    
    # Start controller with --test-mode to enable:
    # - Both endpoint mode (sync both Endpoints and EndpointSlice)
    # - Metrics test features (test_key, test_data)
    "${BIN_DIR}/edgion-controller" \
        -c "$CONTROLLER_CONFIG" \
        --work-dir "${WORK_DIR}" \
        --conf-dir "$CONFIG_DIR" \
        --grpc-listen "0.0.0.0:${CONTROLLER_GRPC_PORT}" \
        --admin-listen "0.0.0.0:${CONTROLLER_ADMIN_PORT}" \
        --test-mode \
        > "${LOG_DIR}/controller.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/controller.pid"
    
    # Waitport
    if ! wait_for_port $CONTROLLER_ADMIN_PORT "edgion-controller" $pid 30; then
        log_error "edgion-controller Startfailed，viewlog: ${LOG_DIR}/controller.log"
        tail -20 "${LOG_DIR}/controller.log" 2>/dev/null || true
        exit 1
    fi
    
    # healthCheck
    if ! wait_for_health "http://127.0.0.1:${CONTROLLER_PROBE_PORT}/health" "edgion-controller" 10; then
        log_error "edgion-controller healthCheckfailed"
        exit 1
    fi

    if ! wait_for_port $CONTROLLER_GRPC_PORT "edgion-controller gRPC" $pid 30; then
        log_error "edgion-controller gRPC Startfailed，viewlog: ${LOG_DIR}/controller.log"
        tail -20 "${LOG_DIR}/controller.log" 2>/dev/null || true
        exit 1
    fi
    
    log_success "edgion-controller Startsuccess (PID: $pid)"
}

# =============================================================================
# Start gateway
# =============================================================================
start_gateway() {
    log_section "Start edgion-gateway"
    local gateway_rust_log="info,pingora_proxy=error,pingora_core=error"
    
    EDGION_ACCESS_LOG="${LOG_DIR}/edgion_access.log" \
    RUST_LOG="${gateway_rust_log}" \
    EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/edgion_access.log" \
    EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
    "${BIN_DIR}/edgion-gateway" \
        -c "$GATEWAY_CONFIG" \
        --work-dir "${WORK_DIR}" \
        --server-addr "${CONF_SYNC_SCHEME}://127.0.0.1:${CONTROLLER_GRPC_PORT}" \
        --integration-testing-mode \
        --admin-listen "0.0.0.0:${GATEWAY_ADMIN_PORT}" \
        > "${LOG_DIR}/gateway.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/gateway.pid"
    
    # Wait for Gateway Admin port (default 12001, overridable via EDGION_TEST_GATEWAY_ADMIN_PORT)
    if ! wait_for_port $GATEWAY_ADMIN_PORT "edgion-gateway" $pid 30; then
        log_error "edgion-gateway Startfailed，viewlog: ${LOG_DIR}/gateway.log"
        tail -20 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi
    if ! wait_for_port $GATEWAY_PROBE_PORT "edgion-gateway probe" $pid 30; then
        log_error "edgion-gateway probe port not ready, view log: ${LOG_DIR}/gateway.log"
        tail -20 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi

    # Wait for Gateway to be fully ready (all caches synced from Controller)
    if ! wait_for_ready "http://127.0.0.1:${GATEWAY_PROBE_PORT}/ready" "edgion-gateway" 60; then
        log_error "edgion-gateway ，viewlog: ${LOG_DIR}/gateway.log"
        tail -30 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi
    
    # Verify LB preload completed (with retry to handle log flush race condition)
    local lb_timeout=15
    local lb_waited=0
    while ! grep -q "LB preload completed" "${LOG_DIR}/gateway.log" 2>/dev/null; do
        if [ $lb_waited -ge $lb_timeout ]; then
            log_error "edgion-gateway LB preload timeout after ${lb_timeout}s"
            tail -50 "${LOG_DIR}/gateway.log" 2>/dev/null || true
            exit 1
        fi
        sleep 1
        ((lb_waited++))
    done
    log_info "LB preload  (waited ${lb_waited}s)"

    # Wait for Pingora server to finish binding all listener ports
    # After LB preload, the server still needs time to process gateways and bind ports
    local listener_wait=0
    local listener_max=10
    while ! grep -q "Listener configured\|Processing gateway.*listeners\b" "${LOG_DIR}/gateway.log" 2>/dev/null; do
        if [ $listener_wait -ge $listener_max ]; then
            break
        fi
        sleep 1
        ((listener_wait++))
    done
    # Extra grace period for port binding after listeners are configured
    sleep 2
    
    log_success "edgion-gateway Startsuccess (PID: $pid)"
}

# =============================================================================
# 
#  edgion-cli apply  Admin API ，FileSystemWriter 
#  Kind_namespace_name.yaml  config 
# =============================================================================
load_base_config() {
    log_section ""
    
    local conf_src="${PROJECT_ROOT}/edgion-tests/integration/conf/base"
    local edgion_cli="${BIN_DIR}/edgion-cli"

    if [ ! -d "$conf_src" ]; then
        log_warning "configdirectory: $conf_src"
        return 0
    fi

    for file in "$conf_src"/*.yaml; do
        if [ -f "$file" ]; then
            local filename=$(basename "$file")
            log_info "Load $filename via API..."
            if "$edgion_cli" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" apply -f "$file" > /dev/null 2>&1; then
                log_success "$filename Loadsuccess"
            else
                log_warning "$filename Loadfailed"
            fi
        fi
    done
    
    log_success "configcompleted"
}

# =============================================================================
# Generatecertificate
# =============================================================================
generate_certs() {
    log_section "GenerateTestcertificate"
    mkdir -p "$GENERATED_SECRET_DIR"
    log_info "Generated Secret output directory: $GENERATED_SECRET_DIR"
    
    # Generate TLS certificate
    if [ -f "${CERTS_DIR}/generate_tls_certs.sh" ]; then
        log_info "Run generate_tls_certs.sh..."
        if EDGION_GENERATED_SECRET_DIR="$GENERATED_SECRET_DIR" bash "${CERTS_DIR}/generate_tls_certs.sh" > /dev/null 2>&1; then
            log_success "TLS certificateGeneratecompleted"
        else
            log_warning "TLS certificatealreadyGenerateSkip"
        fi
    fi
    
    # Generateafter TLS certificate
    if [ -f "${CERTS_DIR}/generate_backend_certs.sh" ]; then
        log_info "Run generate_backend_certs.sh..."
        if EDGION_GENERATED_SECRET_DIR="$GENERATED_SECRET_DIR" bash "${CERTS_DIR}/generate_backend_certs.sh" > /dev/null 2>&1; then
            log_success "after TLS certificateGeneratecompleted"
        else
            log_warning "after TLS certificatealreadyGenerateSkip"
        fi
    fi
    
    # Generate mTLS certificate
    if [ -f "${CERTS_DIR}/generate_mtls_certs.sh" ]; then
        log_info "Run generate_mtls_certs.sh..."
        if EDGION_GENERATED_SECRET_DIR="$GENERATED_SECRET_DIR" bash "${CERTS_DIR}/generate_mtls_certs.sh" > /dev/null 2>&1; then
            log_success "mTLS certificateGeneratecompleted"
        else
            log_warning "mTLS certificatealreadyGenerateSkip"
        fi
    fi
    
}

# =============================================================================
# Load generated Secret configs (runtime only, never write into conf/)
# =============================================================================
load_generated_secrets() {
    log_section "LoadGeneratedSecrets"

    local edgion_cli="${BIN_DIR}/edgion-cli"
    if [ ! -d "$GENERATED_SECRET_DIR" ]; then
        log_warning "Generated Secret directory not found: $GENERATED_SECRET_DIR"
        return 0
    fi

    local files
    files=$(find "$GENERATED_SECRET_DIR" -type f \( -name "*.yaml" -o -name "*.yml" \) | sort)
    if [ -z "$files" ]; then
        log_warning "No generated Secret YAML files found in: $GENERATED_SECRET_DIR"
        return 0
    fi

    local failed=false
    local count=0
    for file in $files; do
        local relative_file="${file#${GENERATED_SECRET_DIR}/}"
        log_info "Load generated Secret $relative_file via API..."
        if "$edgion_cli" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" apply -f "$file" > /dev/null 2>&1; then
            log_success "$relative_file Loadsuccess"
        else
            log_warning "$relative_file Loadfailed"
            failed=true
        fi
        count=$((count + 1))
    done

    if $failed; then
        log_error "Partial generated Secret config load failed"
        exit 1
    fi

    log_success "Generated Secret configs loaded: $count"
}

# =============================================================================
# Loadsuite（directory）
# =============================================================================
get_suites_to_load() {
    local conf_dir="${PROJECT_ROOT}/edgion-tests/integration/conf"
    
    if [ -n "$SUITES" ]; then
        # usespecifysuite
        echo "$SUITES" | tr ',' ' '
    else
        # default： conf directoryalldirectory
        local suites=""
        
        # resource (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute )
        for resource_dir in "${conf_dir}"/*; do
            if [ -d "$resource_dir" ]; then
                local resource_name=$(basename "$resource_dir")
                
                # Skip base directory, Services directory (ACME tests have their own script),
                # and conf-sync-leak-test (standalone stress test, port conflicts with normal suites)
                if [ "$resource_name" = "base" ] || [ "$resource_name" = "Services" ] || [ "$resource_name" = "conf-sync-leak-test" ]; then
                    continue
                fi
                
                # Checkdirectory
                local has_subdir=false
                for subdir in "$resource_dir"/*; do
                    if [ -d "$subdir" ]; then
                        has_subdir=true
                        local subdir_name=$(basename "$subdir")
                        
                        # Checkdirectory
                        local has_deep_subdir=false
                        for deepdir in "$subdir"/*; do
                            if [ -d "$deepdir" ]; then
                                local deepdir_name=$(basename "$deepdir")
                                # Skip dynamic-scenario delta dirs (updates*, delete*): only initial* dirs
                                # are pre-loaded. These names only ever appear under DynamicTest-style
                                # scenario layouts (DynamicTest/, LastKnownGood/, ...), so the glob match
                                # is safe without coupling to a specific parent dir name.
                                if [[ "$deepdir_name" == updates* || "$deepdir_name" == delete* ]]; then
                                    continue
                                fi
                                has_deep_subdir=true
                                suites="$suites ${resource_name}/${subdir_name}/${deepdir_name}"
                            fi
                        done
                        
                        if $has_deep_subdir; then
                            # Also add the parent suite if it has YAML files directly
                            local has_yaml=false
                            for f in "$subdir"/*.yaml "$subdir"/*.yml; do
                                if [ -f "$f" ]; then
                                    has_yaml=true
                                    break
                                fi
                            done
                            if $has_yaml; then
                                suites="$suites ${resource_name}/${subdir_name}"
                            fi
                        else
                            suites="$suites ${resource_name}/${subdir_name}"
                        fi
                    fi
                done
                
                # directory，resourcedirectory
                if ! $has_subdir; then
                    suites="$suites $resource_name"
                fi
            fi
        done
        
        echo $suites
    fi
}

# =============================================================================
# LoadTestconfig
# =============================================================================
load_configs() {
    log_section "LoadTestconfig"
    
    local suites_to_load=$(get_suites_to_load)
    
    if [ -z "$suites_to_load" ]; then
        log_warning "LoadTestconfig"
        return 0
    fi
    
    log_info "Loadconfig: $suites_to_load"
    
    local load_script="${SCRIPT_DIR}/load_conf.sh"
    
    if [ ! -f "$load_script" ]; then
        log_error "load_conf.sh : $load_script"
        exit 1
    fi
    
    for suite in $suites_to_load; do
        log_info "Load $suite config..."
        #  --wait 0  suite ，
        if bash "$load_script" --wait 0 "$suite" 2>&1 | tee -a "${LOG_DIR}/load_config.log"; then
            log_success "$suite configLoadcompleted"
        else
            log_warning "$suite configLoadfailed"
        fi
    done
    
    # ，（Controller ）
    log_info "Waitconfigtake effect (2s)..."
    sleep 2
    
    log_success "allconfigLoadcompleted"
}

# =============================================================================
# verifyresourcesync
# =============================================================================
verify_sync() {
    log_section "verifyresourcesync"
    
    local resource_diff="${BIN_DIR}/resource_diff"
    
    if [ ! -f "$resource_diff" ]; then
        log_warning "resource_diff ，Skipverify"
        return 0
    fi
    
    log_info "Run resource_diff verify Controller  Gateway resourcesync..."
    
    # Retry logic: wait for gateway to fully sync all resources from controller
    # Gateway needs time to fetch data from controller via gRPC
    local max_retries=5
    local retry_delay=2
    local attempt=1
    
    while [ $attempt -le $max_retries ]; do
        # Note: resource_diff now skips ReferenceGrant and Secret by default (--skip-kinds)
        if "$resource_diff" \
            --controller-url "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" \
            --controller-probe-url "http://127.0.0.1:${CONTROLLER_PROBE_PORT}" \
            --gateway-url "http://127.0.0.1:${GATEWAY_ADMIN_PORT}" \
            --gateway-probe-url "http://127.0.0.1:${GATEWAY_PROBE_PORT}" \
            > "${LOG_DIR}/resource_diff.log" 2>&1; then
            log_success "resourcesyncverifypassed"
            return 0
        fi
        
        if [ $attempt -lt $max_retries ]; then
            log_info "verify attempt $attempt failed, retrying in ${retry_delay}s..."
            sleep $retry_delay
        fi
        ((attempt++))
    done
    
    log_error "resourcesyncverifyfailed after $max_retries attempts，viewlog: ${LOG_DIR}/resource_diff.log"
    tail -20 "${LOG_DIR}/resource_diff.log" 2>/dev/null || true
    exit 1
}

# =============================================================================
# Workdirectoryinfo
# =============================================================================
save_info() {
    # currentWorkdirectorypath
    mkdir -p "${PROJECT_ROOT}/tmp/integration_testing"
    echo "$WORK_DIR" > "${PROJECT_ROOT}/tmp/integration_testing/.current"
    
    # info
    cat > "${WORK_DIR}/info.txt" << EOF
Edgion Integration Testing
===========================
Started: $(date)
Work Dir: ${WORK_DIR}
Suites: $(get_suites_to_load)

Services:
  - test_server:       PID $(cat ${PID_DIR}/test_server.pid), http://127.0.0.1:${TEST_SERVER_HTTP_PORT}
  - edgion-controller: PID $(cat ${PID_DIR}/controller.pid), http://127.0.0.1:${CONTROLLER_ADMIN_PORT}
  - edgion-gateway:    PID $(cat ${PID_DIR}/gateway.pid), http://127.0.0.1:${GATEWAY_HTTP_PORT}

Logs:
  - ${LOG_DIR}/test_server.log
  - ${LOG_DIR}/controller.log
  - ${LOG_DIR}/gateway.log
  - ${LOG_DIR}/edgion_access.log

Generated Secrets:
  - ${GENERATED_SECRET_DIR}

Stop: ./edgion-tests/integration/scripts/utils/kill_all.sh
EOF
}

# =============================================================================
# 
# =============================================================================
main() {
    # Parseargs
    while [[ $# -gt 0 ]]; do
        case $1 in
        --suites)
            SUITES="$2"
            shift 2
            ;;
        --controller-grpc-port)
            CONTROLLER_GRPC_PORT="$2"
            shift 2
            ;;
        -h|--help)
            show_help
            exit 0
                ;;
            *)
                log_error "unknownoptions: $1"
                show_help
                exit 1
                ;;
        esac
    done
    
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Edgion TestserviceStart（configLoad）${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "Project:  ${PROJECT_ROOT}"
    echo -e "Work Dir: ${WORK_DIR}"
    echo -e "Test Mode: ${GREEN}enabled${NC} (Both endpoint mode + metrics test)"
    if [ -n "$SUITES" ]; then
        echo -e "Suites:   ${SUITES}"
    else
        echo -e "Suites:   all (auto)"
    fi
    
    # : Cleanupallprocess
    kill_all_processes

    # Clear any cached edgion-cli token before this run. The fixed pre-shared
    # admin token is re-persisted to ~/.edgion/token after the Controller is up
    # (see persist_admin_token), so edgion-cli and test_client pick it up via the
    # file fallback even when EDGION_TOKEN is not exported into their environment.
    rm -f ~/.edgion/token

    # : Checkbinaryfile
    check_binaries
    
    # : Workdirectory
    log_section "Workdirectory"
    mkdir -p "$LOG_DIR" "$PID_DIR" "$CONFIG_DIR" "$GENERATED_SECRET_DIR"
    log_success "Workdirectorycompleted: $WORK_DIR"
    
    # :  CRD schemas 
    if [ -d "${PROJECT_ROOT}/config/crd" ]; then
        cp -r "${PROJECT_ROOT}/config/crd" "$CONFIG_DIR/"
        log_success "CRD schemas completed"
    else
        log_error "CRD schemas : ${PROJECT_ROOT}/config/crd"
        exit 1
    fi
    
    # : Generatecertificate（mustconfig，willGenerate Secret file）
    generate_certs

    # conf_sync channel: default to mTLS (dogfood the production default).
    # Copy base configs into the work dir and append the security block to the
    # COPIES — never mutate the checked-in config/edgion-*.yaml. Set
    # EDGION_CONF_SYNC_PLAINTEXT=1 to keep plaintext (used by the metric test).
    mkdir -p "${CONFIG_DIR}"
    cp "${PROJECT_ROOT}/config/edgion-controller.yaml" "${CONFIG_DIR}/edgion-controller.yaml"
    cp "${PROJECT_ROOT}/config/edgion-gateway.yaml" "${CONFIG_DIR}/edgion-gateway.yaml"
    CONTROLLER_CONFIG="${CONFIG_DIR}/edgion-controller.yaml"
    GATEWAY_CONFIG="${CONFIG_DIR}/edgion-gateway.yaml"
    if [ "${EDGION_CONF_SYNC_PLAINTEXT:-0}" = "1" ]; then
        echo -e "${YELLOW}conf_sync: PLAINTEXT mode (EDGION_CONF_SYNC_PLAINTEXT=1)${NC}"
        CONF_SYNC_SCHEME="http"
    else
        conf_sync_generate_and_distribute_certs "${WORK_DIR}"
        conf_sync_append_single_cert_block "${CONTROLLER_CONFIG}" "${GATEWAY_CONFIG}"
        CONF_SYNC_SCHEME="https"
        echo -e "${GREEN}conf_sync: mTLS mode (default)${NC}"
    fi

    # : Start test_server
    start_test_server
    
    # : Start controller
    start_controller
    
    # :  ConfigServer
    if ! wait_for_ready "http://127.0.0.1:${CONTROLLER_PROBE_PORT}/ready" "edgion-controller" 30; then
        log_error "edgion-controller ConfigServer "
        exit 1
    fi

    # Persist the pre-shared admin token to ~/.edgion/token so edgion-cli (file
    # fallback) and test_client (~/.edgion/token fallback) authenticate even when
    # EDGION_TOKEN is not exported into their environment. Best-effort.
    "${BIN_DIR}/edgion-cli" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" \
        login --token "${EDGION_ADMIN_TOKEN}" > /dev/null 2>&1 || true

    # : （ API）
    load_base_config

    # : LoadTestconfig（ API）
    load_configs

    # : Load generated Secret config（ API, override templates in conf/）
    load_generated_secrets
    
    # : Start gateway
    start_gateway
    
    # : verifyresourcesync
    verify_sync
    
    # info
    save_info
    
    # completed
    log_section "Startcompleted"
    log_success "allserviceStartsuccess！"
    echo ""
    echo "Workdirectory: ${WORK_DIR}"
    echo ""
    echo "servicestatus:"
    echo "  - test_server:       http://127.0.0.1:${TEST_SERVER_HTTP_PORT} (PID: $(cat ${PID_DIR}/test_server.pid))"
    echo "  - edgion-controller: http://127.0.0.1:${CONTROLLER_ADMIN_PORT} (PID: $(cat ${PID_DIR}/controller.pid))"
    echo "  - edgion-gateway:    http://127.0.0.1:${GATEWAY_HTTP_PORT} (PID: $(cat ${PID_DIR}/gateway.pid))"
    echo ""
    echo "logdirectory: ${LOG_DIR}"
    echo "configdirectory: ${CONFIG_DIR}"
    echo "generated secrets: ${GENERATED_SECRET_DIR}"
    echo ""
    echo "Stopservice: ./edgion-tests/integration/scripts/utils/kill_all.sh"
    echo ""
    
    # Workdirectorypath（script）
    echo "$WORK_DIR"
}

main "$@"
