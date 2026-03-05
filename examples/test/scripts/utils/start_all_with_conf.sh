#!/bin/bash
# =============================================================================
# Startall Edgion TestserviceLoadconfig
# Start: test_server -> controller ->  ->  -> gateway -> verify
# 
#  Admin API (edgion-ctl apply) ，FileSystemWriter 
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
CERTS_DIR="${SCRIPT_DIR}/../certs"

# Workdirectory
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/testing_${TIMESTAMP}"
GENERATED_SECRET_DIR="${WORK_DIR}/generated-secrets"

#  WORK_DIR scriptuse
export EDGION_WORK_DIR="$WORK_DIR"
export EDGION_GENERATED_SECRET_DIR="$GENERATED_SECRET_DIR"

# directory
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"
CONFIG_DIR="${WORK_DIR}/config"

# configfile
CONTROLLER_CONFIG="${PROJECT_ROOT}/config/edgion-controller.toml"
GATEWAY_CONFIG="${PROJECT_ROOT}/config/edgion-gateway.toml"

# serviceport
TEST_SERVER_HTTP_PORT=30001
CONTROLLER_ADMIN_PORT=5800
# Gateway portuse http Testsuiteport（31000）
GATEWAY_HTTP_PORT=31000
GATEWAY_ADMIN_PORT=5900

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
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/edgion-controller" ]; then
        log_error "edgion-controller Build"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/edgion-gateway" ]; then
        log_error "edgion-gateway Build"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/examples/test_server" ]; then
        log_error "test_server Build"
        missing=true
    fi
    
    if [ ! -f "${PROJECT_ROOT}/target/debug/examples/resource_diff" ]; then
        log_error "resource_diff Build"
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

# =============================================================================
# Start test_server
# =============================================================================
start_test_server() {
    log_section "Start test_server"
    
    # Backend TLS certificatepath
    BACKEND_CERT="${PROJECT_ROOT}/examples/test/certs/backend/server.crt"
    BACKEND_KEY="${PROJECT_ROOT}/examples/test/certs/backend/server.key"
    BACKEND_MTLS_CLIENT_CA="${PROJECT_ROOT}/examples/test/certs/mtls/client-ca.crt"
    
    # Checkafter TLS certificate
    local https_backend_args=""
    if [ -f "$BACKEND_CERT" ] && [ -f "$BACKEND_KEY" ]; then
        https_backend_args="--https-backend-port 30051 --cert-file $BACKEND_CERT --key-file $BACKEND_KEY"
        log_info " HTTPS afterport 30051"
    else
        log_warning "Backend TLS certificate，Skip HTTPS after"
    fi

    local https_backend_mtls_args=""
    if [ -f "$BACKEND_CERT" ] && [ -f "$BACKEND_KEY" ] && [ -f "$BACKEND_MTLS_CLIENT_CA" ]; then
        https_backend_mtls_args="--https-backend-mtls-port 30052 --client-ca-file $BACKEND_MTLS_CLIENT_CA"
        log_info " HTTPS mTLS afterport 30052"
    else
        log_warning "Backend mTLS ，Skip HTTPS mTLS after"
    fi
    
    "${PROJECT_ROOT}/target/debug/examples/test_server" \
        --http-ports "30001,30002,30003" \
        --grpc-ports "30021,30022,30023" \
        --websocket-port 30005 \
        --tcp-port 30010 \
        --udp-port 30011 \
        --auth-port 30040 \
        --log-level info \
        $https_backend_args \
        $https_backend_mtls_args \
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
    
    # Start controller with --test-mode to enable:
    # - Both endpoint mode (sync both Endpoints and EndpointSlice)
    # - Metrics test features (test_key, test_data)
    "${PROJECT_ROOT}/target/debug/edgion-controller" \
        -c "$CONTROLLER_CONFIG" \
        --work-dir "${WORK_DIR}" \
        --conf-dir "$CONFIG_DIR" \
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
    if ! wait_for_health "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/health" "edgion-controller" 10; then
        log_error "edgion-controller healthCheckfailed"
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
    
    EDGION_ACCESS_LOG="${LOG_DIR}/access.log" \
    RUST_LOG="${gateway_rust_log}" \
    EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log" \
    "${PROJECT_ROOT}/target/debug/edgion-gateway" \
        -c "$GATEWAY_CONFIG" \
        --work-dir "${WORK_DIR}" \
        --integration-testing-mode \
        > "${LOG_DIR}/gateway.log" 2>&1 &
    
    local pid=$!
    echo $pid > "${PID_DIR}/gateway.pid"
    
    # Wait for Gateway Admin port (always 5900, regardless of test suite listener ports)
    if ! wait_for_port $GATEWAY_ADMIN_PORT "edgion-gateway" $pid 30; then
        log_error "edgion-gateway Startfailed，viewlog: ${LOG_DIR}/gateway.log"
        tail -20 "${LOG_DIR}/gateway.log" 2>/dev/null || true
        exit 1
    fi
    
    # Wait for Gateway to be fully ready (all caches synced from Controller)
    if ! wait_for_ready "http://127.0.0.1:${GATEWAY_ADMIN_PORT}/ready" "edgion-gateway" 60; then
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
    
    log_success "edgion-gateway Startsuccess (PID: $pid)"
}

# =============================================================================
# 
#  edgion-ctl apply  Admin API ，FileSystemWriter 
#  Kind_namespace_name.yaml  config 
# =============================================================================
load_base_config() {
    log_section ""
    
    local conf_src="${PROJECT_ROOT}/examples/test/conf/base"
    local edgion_ctl="${PROJECT_ROOT}/target/debug/edgion-ctl"
    
    if [ ! -d "$conf_src" ]; then
        log_warning "configdirectory: $conf_src"
        return 0
    fi
    
    for file in "$conf_src"/*.yaml; do
        if [ -f "$file" ]; then
            local filename=$(basename "$file")
            log_info "Load $filename via API..."
            if "$edgion_ctl" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" apply -f "$file" > /dev/null 2>&1; then
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

    local edgion_ctl="${PROJECT_ROOT}/target/debug/edgion-ctl"
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
        if "$edgion_ctl" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" apply -f "$file" > /dev/null 2>&1; then
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
    local conf_dir="${PROJECT_ROOT}/examples/test/conf"
    
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
                
                # Skip base directory and Services directory (ACME tests have their own script)
                if [ "$resource_name" = "base" ] || [ "$resource_name" = "Services" ]; then
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
                                # Skip DynamicTest/updates and DynamicTest/delete
                                if [[ "$subdir_name" == "DynamicTest" && ("$deepdir_name" == "updates" || "$deepdir_name" == "delete") ]]; then
                                    continue
                                fi
                                has_deep_subdir=true
                                suites="$suites ${resource_name}/${subdir_name}/${deepdir_name}"
                            fi
                        done
                        
                        if ! $has_deep_subdir; then
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
    
    local resource_diff="${PROJECT_ROOT}/target/debug/examples/resource_diff"
    
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
            --gateway-url "http://127.0.0.1:${GATEWAY_ADMIN_PORT}" \
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
    mkdir -p "${PROJECT_ROOT}/integration_testing"
    echo "$WORK_DIR" > "${PROJECT_ROOT}/integration_testing/.current"
    
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
  - ${LOG_DIR}/access.log

Generated Secrets:
  - ${GENERATED_SECRET_DIR}

Stop: ./examples/test/scripts/utils/kill_all.sh
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
    
    # : Start test_server
    start_test_server
    
    # : Start controller
    start_controller
    
    # :  ConfigServer 
    if ! wait_for_ready "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}/ready" "edgion-controller" 30; then
        log_error "edgion-controller ConfigServer "
        exit 1
    fi
    
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
    echo "Stopservice: ./examples/test/scripts/utils/kill_all.sh"
    echo ""
    
    # Workdirectorypath（script）
    echo "$WORK_DIR"
}

main "$@"
