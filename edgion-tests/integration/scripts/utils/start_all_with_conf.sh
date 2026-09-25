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
source "${SCRIPT_DIR}/conf_sync_certs.sh"

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
GATEWAY_BIN="${EDGION_GATEWAY_BIN:-${BIN_DIR}/edgion-gateway}"

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
CAPABILITY_REGISTRY="${PROJECT_ROOT}/edgion-tests/integration/capabilities.tsv"
EXT_PROC_EXTERNAL_BIN=""
EXT_PROC_EXTERNAL_GRPC_PORT=""
EXT_PROC_EXTERNAL_ADMIN_PORT=""
EXT_PROC_EXTERNAL_TLS_PORT=""
EXT_PROC_EXTERNAL_MTLS_PORT=""
EXT_PROC_EXTERNAL_CONFIG_TEMPLATE=""
EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR="${WORK_DIR}/ext-proc-external-tls"
EXT_PROC_EXTERNAL_PID=""
EXT_PROC_EXTERNAL_STARTUP_OWNED=false

gateway_has_capability() {
    local capability="$1"
    echo "${EDGION_TEST_GATEWAY_CAPABILITIES:-}" | grep -q "^${capability}=enabled$"
}

load_gateway_capabilities() {
    local capabilities
    if ! capabilities=$("$GATEWAY_BIN" --print-build-capabilities); then
        log_error "Failed to read build capabilities from ${GATEWAY_BIN}"
        return 1
    fi
    if ! echo "$capabilities" | grep -q '^format=edgion-build-capabilities/v1$'; then
        log_error "Unsupported Gateway capability format from ${GATEWAY_BIN}"
        return 1
    fi
    echo "$capabilities" | grep -q '^body-buffering=enabled$' || {
        log_error "Gateway does not report body-buffering=enabled"
        return 1
    }
    EDGION_TEST_GATEWAY_CAPABILITIES="$capabilities"
    export EDGION_TEST_GATEWAY_CAPABILITIES
}

validate_capability_registry() {
    if ! EDGION_CAPABILITY_REGISTRY="$CAPABILITY_REGISTRY" \
        "${BIN_DIR}/test_client" --list-dynamic-scenarios > /dev/null; then
        log_error "Integration capability registry validation failed"
        return 1
    fi
}

suite_is_supported() {
    local suite="$1"
    local kind id capability
    while IFS=$'\t' read -r kind id capability; do
        case "$kind" in
            ""|\#*) continue ;;
        esac
        if [ "$kind" = "suite" ]; then
            case "$suite" in
                "$id"|"$id"/*)
                    gateway_has_capability "$capability"
                    return $?
                    ;;
            esac
        fi
    done < "$CAPABILITY_REGISTRY"
    return 0
}

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
    echo "  --ext-proc-external-bin <path>  absolute external ExtProc fixture binary"
    echo "  --ext-proc-external-grpc-port <port>  fixture loopback gRPC port"
    echo "  --ext-proc-external-admin-port <port> fixture loopback admin port"
    echo "  --ext-proc-external-tls-port <port> fixture loopback TLS gRPC port"
    echo "  --ext-proc-external-mtls-port <port> fixture loopback mTLS gRPC port"
    echo "  --ext-proc-external-config-template <path> absolute full-only YAML template"
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
    pkill -9 -f "debug/edgion-controller" 2>/dev/null && log_info "alreadyStop edgion-controller" || true
    pkill -9 -f "debug/edgion-gateway" 2>/dev/null && log_info "alreadyStop edgion-gateway" || true
    pkill -9 -f "debug/test_server" 2>/dev/null && log_info "alreadyStop test_server" || true
    
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
    
    if [ ! -x "${GATEWAY_BIN}" ]; then
        log_error "Gateway binary not found or not executable: ${GATEWAY_BIN}"
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

validate_port() {
    local port="$1"
    local label="$2"
    case "$port" in
        ''|*[!0-9]*)
            log_error "${label} must be an integer in 1..65535"
            return 1
            ;;
    esac
    if [ "$port" -lt 1 ] || [ "$port" -gt 65535 ]; then
        log_error "${label} must be an integer in 1..65535"
        return 1
    fi
}

stop_external_ext_proc_pid() {
    local pid="$1"
    if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
        return 0
    fi
    kill -TERM "$pid" 2>/dev/null || true
    local waited=0
    while [ "$waited" -lt 5 ] && kill -0 "$pid" 2>/dev/null; do
        sleep 1
        waited=$((waited + 1))
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
}

cleanup_external_ext_proc_on_exit() {
    if $EXT_PROC_EXTERNAL_STARTUP_OWNED && [ -n "$EXT_PROC_EXTERNAL_PID" ]; then
        log_warning "Startup failed; stopping external ExtProc fixture PID ${EXT_PROC_EXTERNAL_PID}"
        stop_external_ext_proc_pid "$EXT_PROC_EXTERNAL_PID"
    fi
}

external_ext_proc_ready() {
    local response
    if ! response=$(curl --max-time 2 -fsS \
        "http://127.0.0.1:${EXT_PROC_EXTERNAL_ADMIN_PORT}/readyz" 2>/dev/null); then
        return 1
    fi
    printf '%s' "$response" | python3 -c '
import json
import sys

expected = {
    "status": "ready",
    "fixture": "edgion-ext-proc-external",
    "protocol_version": "v1",
    "go_control_plane_envoy": "v1.37.0",
    "fixture_stream_capacity": 96,
    "tls_capable": True,
    "mtls_capable": True,
    "tls_ready": True,
    "mtls_ready": True,
}
try:
    value = json.load(sys.stdin)
except (json.JSONDecodeError, UnicodeDecodeError):
    raise SystemExit(1)
if not isinstance(value, dict):
    raise SystemExit(1)
for key, expected_value in expected.items():
    if value.get(key) != expected_value:
        raise SystemExit(1)
'
}

start_external_ext_proc() {
    [ -z "$EXT_PROC_EXTERNAL_BIN" ] && return 0

    log_section "Start external ExtProc fixture"
    if ! check_port_conflict "$EXT_PROC_EXTERNAL_GRPC_PORT" "external ExtProc gRPC" \
        || ! check_port_conflict "$EXT_PROC_EXTERNAL_ADMIN_PORT" "external ExtProc admin" \
        || ! check_port_conflict "$EXT_PROC_EXTERNAL_TLS_PORT" "external ExtProc TLS gRPC" \
        || ! check_port_conflict "$EXT_PROC_EXTERNAL_MTLS_PORT" "external ExtProc mTLS gRPC"; then
        return 1
    fi

    "$EXT_PROC_EXTERNAL_BIN" \
        --grpc-listen "127.0.0.1:${EXT_PROC_EXTERNAL_GRPC_PORT}" \
        --admin-listen "127.0.0.1:${EXT_PROC_EXTERNAL_ADMIN_PORT}" \
        --tls-listen "127.0.0.1:${EXT_PROC_EXTERNAL_TLS_PORT}" \
        --mtls-listen "127.0.0.1:${EXT_PROC_EXTERNAL_MTLS_PORT}" \
        --tls-material-dir "$EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR" \
        > "${LOG_DIR}/ext_proc_external_processor.log" 2>&1 &
    EXT_PROC_EXTERNAL_PID=$!
    printf '%s\n' "$EXT_PROC_EXTERNAL_PID" > "${PID_DIR}/ext_proc_external_processor.pid"
    printf '%s\n' "$EXT_PROC_EXTERNAL_BIN" > "${WORK_DIR}/ext-proc-external-bin"
    EXT_PROC_EXTERNAL_STARTUP_OWNED=true
    trap cleanup_external_ext_proc_on_exit EXIT

    local elapsed=0
    while [ "$elapsed" -lt 30 ]; do
        if ! kill -0 "$EXT_PROC_EXTERNAL_PID" 2>/dev/null; then
            log_error "External ExtProc fixture exited during startup"
            tail -20 "${LOG_DIR}/ext_proc_external_processor.log" 2>/dev/null || true
            return 1
        fi
        if external_ext_proc_ready; then
            log_success "External ExtProc fixture ready (PID: ${EXT_PROC_EXTERNAL_PID})"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    log_error "External ExtProc fixture readiness timed out after 30s"
    tail -20 "${LOG_DIR}/ext_proc_external_processor.log" 2>/dev/null || true
    return 1
}

render_and_load_external_ext_proc_config() {
    [ -z "$EXT_PROC_EXTERNAL_BIN" ] && return 0

    log_section "Load external ExtProc config"
    local rendered="${CONFIG_DIR}/ext_proc_external.yaml"
    local target="127.0.0.1:${EXT_PROC_EXTERNAL_GRPC_PORT}"
    local tls_target="127.0.0.1:${EXT_PROC_EXTERNAL_TLS_PORT}"
    local mtls_target="127.0.0.1:${EXT_PROC_EXTERNAL_MTLS_PORT}"
    local token="__EXTERNAL_EXT_PROC_TARGET__"
    local tls_token="__EXTERNAL_EXT_PROC_TLS_TARGET__"
    local mtls_token="__EXTERNAL_EXT_PROC_MTLS_TARGET__"
    local required_token
    for required_token in "$token" "$tls_token" "$mtls_token"; do
        if ! grep -Fq "$required_token" "$EXT_PROC_EXTERNAL_CONFIG_TEMPLATE"; then
            log_error "External ExtProc config template does not contain ${required_token}"
            return 1
        fi
    done
    if ! EXT_PROC_EXTERNAL_RENDER_TARGET="$target" \
        EXT_PROC_EXTERNAL_RENDER_TLS_TARGET="$tls_target" \
        EXT_PROC_EXTERNAL_RENDER_MTLS_TARGET="$mtls_target" \
        awk -v token="$token" -v tls_token="$tls_token" -v mtls_token="$mtls_token" '
        {
            line = $0
            while ((position = index(line, token)) != 0) {
                line = substr(line, 1, position - 1) ENVIRON["EXT_PROC_EXTERNAL_RENDER_TARGET"] substr(line, position + length(token))
            }
            while ((position = index(line, tls_token)) != 0) {
                line = substr(line, 1, position - 1) ENVIRON["EXT_PROC_EXTERNAL_RENDER_TLS_TARGET"] substr(line, position + length(tls_token))
            }
            while ((position = index(line, mtls_token)) != 0) {
                line = substr(line, 1, position - 1) ENVIRON["EXT_PROC_EXTERNAL_RENDER_MTLS_TARGET"] substr(line, position + length(mtls_token))
            }
            print line
        }
    ' "$EXT_PROC_EXTERNAL_CONFIG_TEMPLATE" > "$rendered"; then
        log_error "Failed to render external ExtProc config"
        return 1
    fi
    if grep -Eq '__EXTERNAL_EXT_PROC_[A-Z_]*TARGET__' "$rendered"; then
        log_error "External ExtProc config still contains an unresolved target placeholder"
        return 1
    fi
    local server_ca_manifest="${EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR}/ext-proc-external-server-ca.yaml"
    local initial_client_manifest="${EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR}/ext-proc-external-client-initial.yaml"
    if [ ! -f "$server_ca_manifest" ] || [ ! -f "$initial_client_manifest" ]; then
        log_error "External ExtProc TLS Secret manifests are missing"
        return 1
    fi
    if ! "${BIN_DIR}/edgion-cli" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" \
        apply -f "$server_ca_manifest" -f "$initial_client_manifest" > /dev/null; then
        log_error "External ExtProc TLS Secret load failed"
        return 1
    fi
    if ! "${BIN_DIR}/edgion-cli" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" \
        apply -f "$rendered"; then
        log_error "External ExtProc config load failed"
        return 1
    fi
    log_success "External ExtProc config loaded: ${rendered}"
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
    BACKEND_CERT_WILDCARD="${PROJECT_ROOT}/edgion-tests/certs/backend/server-wildcard.crt"
    BACKEND_KEY_WILDCARD="${PROJECT_ROOT}/edgion-tests/certs/backend/server-wildcard.key"

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
        https_backend_mtls_args="--https-backend-mtls-port 30052 --ext-proc-mtls-port 30043 --client-ca-file $BACKEND_MTLS_CLIENT_CA"
        log_info " HTTPS mTLS afterport 30052"
        log_info " ext_proc mTLS port 30043"
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

    # EdgionBackendTrafficPolicy/UpstreamAuthority scenario: a single
    # *.sandbox.internal wildcard cert covers every tenant subdomain.
    local https_backend_wildcard_args=""
    if [ -f "$BACKEND_CERT_WILDCARD" ] && [ -f "$BACKEND_KEY_WILDCARD" ]; then
        https_backend_wildcard_args="--https-backend-wildcard-port 30028 --wildcard-cert $BACKEND_CERT_WILDCARD --wildcard-key $BACKEND_KEY_WILDCARD"
        log_info "HTTPS wildcard (*.sandbox.internal) backend on port 30028"
    else
        log_warning "Backend wildcard cert not found, skipping wildcard backend (port 30028)"
    fi

    "${BIN_DIR}/test_server" \
        --http-ports "30001,30002,30003" \
        --grpc-ports "30021,30022,30023" \
        --websocket-port 30005 \
        --tcp-port 30010 \
        --l4-duplex-port 30032 \
        --l4-duplex-tls-port 30033 \
        --udp-port 30011 \
        --tcp-pp2-port 30012 \
        --auth-port 30040 \
        --ext-proc-port 30041 \
        --h2-trailer-malformed-port 30025 \
        --transport-h1-port 30030 \
        --transport-h2-port 30031 \
        --log-level info \
        $ext_proc_tls_args \
        $https_backend_args \
        $https_backend_mtls_args \
        $https_backend_multi_sni_args \
        $https_backend_wildcard_args \
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
    "${GATEWAY_BIN}" \
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
    
    # Verify conf-sync caches are ready (selectors compile with each generation;
    # there is no separate LB preload pass).
    local lb_timeout=15
    local lb_waited=0
    while ! grep -q "All caches are ready" "${LOG_DIR}/gateway.log" 2>/dev/null; do
        if [ $lb_waited -ge $lb_timeout ]; then
            log_error "edgion-gateway cache-ready timeout after ${lb_timeout}s"
            tail -50 "${LOG_DIR}/gateway.log" 2>/dev/null || true
            exit 1
        fi
        sleep 1
        # NOT `((lb_waited++))`: post-increment evaluates to the OLD value, so the
        # first pass evaluates to 0, which makes `((...))` return exit status 1 and
        # `set -e` (line 10) kill the script. bash 3.2 (macOS) does not enforce -e
        # here, so this only ever fired on Linux/bash 5 — i.e. in CI, never on a dev
        # Mac. An assignment always returns 0.
        lb_waited=$((lb_waited + 1))
    done
    log_info "caches ready (waited ${lb_waited}s)"

    # Listener configuration may take longer than the early cache-ready signal.
    # Wait until all listeners are configured and Pingora actually starts;
    # "Processing gateway" precedes binding and is not a readiness signal.
    local listener_wait=0
    local listener_max=60
    while ! grep -q "Starting Pingora server" "${LOG_DIR}/gateway.log" 2>/dev/null; do
        if ! kill -0 "$pid" 2>/dev/null; then
            log_error "edgion-gateway exited before starting Pingora"
            return 1
        fi
        if [ $listener_wait -ge $listener_max ]; then
            log_error "edgion-gateway listener setup timed out after ${listener_max}s"
            return 1
        fi
        sleep 1
        # Same bash-3.2-vs-5 `set -e` trap as lb_waited above.
        listener_wait=$((listener_wait + 1))
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

    local apply_args=()
    local count=0
    for file in "$conf_src"/*.yaml; do
        if [ -f "$file" ]; then
            apply_args+=("-f" "$file")
            count=$((count + 1))
        fi
    done

    if [ "$count" -eq 0 ]; then
        log_warning "No base YAML files found in: $conf_src"
        return 1
    fi

    log_info "Loading $count base config files with one ordered CLI batch..."
    if ! "$edgion_cli" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" \
        apply "${apply_args[@]}"; then
        log_error "Base config load failed"
        return 1
    fi

    log_success "Base config load completed"
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

    # Generate LDAP TLS Secrets from the same ephemeral CA used by the Docker
    # dependency. Static fixtures cannot stay in sync with gitignored cert files.
    local ldap_tls_cert_script="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/ldap-openldap-tls/make-test-certs.sh"
    if [ -f "$ldap_tls_cert_script" ]; then
        log_info "Run LDAP TLS certificate preparation..."
        if EDGION_GENERATED_SECRET_DIR="$GENERATED_SECRET_DIR" bash "$ldap_tls_cert_script" > /dev/null 2>&1; then
            log_success "LDAP TLS certificate preparation completed"
        else
            log_error "LDAP TLS certificate preparation failed"
            exit 1
        fi
    fi

    local redis_tls_selected=false
    if [ "${EDGION_TEST_ENABLE_REDIS_TLS:-0}" = "1" ] \
        || [ -z "$SUITES" ] \
        || [[ ",${SUITES}," == *",LinkSys/RedisTls,"* ]]; then
        redis_tls_selected=true
    fi
    if $redis_tls_selected; then
        # Generate Redis TLS Secrets from the same ephemeral CA mounted into the
        # Redis TLS and mTLS containers.
        local redis_tls_cert_script="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/redis/make-test-certs.sh"
        if [ -f "$redis_tls_cert_script" ]; then
            log_info "Run Redis TLS certificate preparation..."
            if EDGION_GENERATED_SECRET_DIR="$GENERATED_SECRET_DIR" bash "$redis_tls_cert_script" > /dev/null 2>&1; then
                log_success "Redis TLS certificate preparation completed"
            else
                log_error "Redis TLS certificate preparation failed"
                exit 1
            fi
        else
            log_error "Redis TLS certificate script not found: ${redis_tls_cert_script}"
            exit 1
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

    local count=0
    local apply_args=()
    for file in $files; do
        local relative_file="${file#${GENERATED_SECRET_DIR}/}"
        log_info "Queue generated Secret: $relative_file"
        apply_args+=("-f" "$file")
        count=$((count + 1))
    done

    log_info "Loading $count generated Secret files with one ordered CLI batch..."
    if ! "$edgion_cli" --server "http://127.0.0.1:${CONTROLLER_ADMIN_PORT}" \
        apply "${apply_args[@]}" > /dev/null; then
        log_error "Generated Secret config load failed"
        return 1
    fi

    log_success "Generated Secret configs loaded: $count"
}

# =============================================================================
# Loadsuite（directory）
# =============================================================================
get_suites_to_load() {
    local conf_dir="${PROJECT_ROOT}/edgion-tests/integration/conf"
    local suites=""
    
    if [ -n "$SUITES" ]; then
        # usespecifysuite
        suites=$(echo "$SUITES" | tr ',' ' ')
    else
        # default： conf directoryalldirectory
        # resource (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute )
        for resource_dir in "${conf_dir}"/*; do
            if [ -d "$resource_dir" ]; then
                local resource_name=$(basename "$resource_dir")
                
                # Skip base directory, Services directory (ACME tests have their own script),
                # conf-sync-leak-test (standalone stress test, port conflicts with normal suites),
                # ProxyProtocolInbound / ProxyProtocolChain are excluded for the same
                # fixtures replace the shared GatewayClass and their listeners
                # demand a PROXY header on every connection, so loading them into the shared
                # stack would make those ports unusable by every other suite. Both boot their
                # own stack from their own conf dir. ProxyProtocolChain's GatewayClass is the
                # more dangerous of the two — it carries a parametersRef to `chain-egc`, so
                # loading it here would replace base/GatewayClass.yaml's pointer to
                # `example-gateway` and silently repoint every GatewayConfig-driven suite.
                if [ "$resource_name" = "base" ] || [ "$resource_name" = "Services" ] || [ "$resource_name" = "conf-sync-leak-test" ] || [ "$resource_name" = "ProxyProtocolInbound" ] || [ "$resource_name" = "ProxyProtocolChain" ]; then
                    continue
                fi
                
                # Checkdirectory
                local has_subdir=false
                for subdir in "$resource_dir"/*; do
                    if [ -d "$subdir" ]; then
                        has_subdir=true
                        local subdir_name=$(basename "$subdir")

                        # OTLP owns a standalone process topology. Its TLS listener
                        # overlaps HTTPRoute/Delegation on port 31932, so only load
                        # these fixtures when explicitly requested via --suites.
                        if [ "$resource_name/$subdir_name" = "LinkSys/Otlp" ]; then
                            continue
                        fi
                        
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
        
    fi

    local supported=""
    local suite
    for suite in $suites; do
        # Base is loaded exactly once by load_base_config(). Target inference
        # can also name the same dependency suite twice (for example an exact
        # HTTPRoute/Basic run). Preserve first-seen order while dropping both
        # forms of redundant API work.
        if [ "$suite" = "base" ]; then
            continue
        fi
        case " $supported " in
            *" $suite "*) continue ;;
        esac
        if suite_is_supported "$suite"; then
            supported="${supported} ${suite}"
        else
            log_warning "Skip unsupported suite config: ${suite}" >&2
        fi
    done
    echo "$supported"
}

# =============================================================================
# LoadTestconfig
# =============================================================================
load_configs() {
    log_section "LoadTestconfig"

    local suites_to_load=$(get_suites_to_load)
    local failed=false

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
        # Keep pipefail local: both the loader and the log sink must succeed.
        if (set -o pipefail; bash "$load_script" --wait 0 "$suite" 2>&1 | tee -a "${LOG_DIR}/load_config.log"); then
            log_success "$suite configLoadcompleted"
        else
            log_error "Config load or log write failed for ${suite}; see ${LOG_DIR}/load_config.log and the output above"
            failed=true
        fi
    done

    if $failed; then
        log_error "One or more config suites failed to load"
        return 1
    fi

    # resource_diff below is the synchronization barrier. Starting it
    # immediately avoids an unconditional delay on fast machines while keeping
    # bounded retries for slower Controller/Gateway convergence.
    log_info "Config submission complete; resource_diff will wait for convergence"

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
        # Safe today only because `attempt` starts at 1, so `((attempt++))` never
        # evaluates to 0 — but that is a property of the initial value, not of the
        # code. Written as an assignment so it stays correct if that ever changes.
        attempt=$((attempt + 1))
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
    printf '%s\n' "$GATEWAY_BIN" > "${WORK_DIR}/gateway-bin"
    printf '%s\n' "$EDGION_TEST_GATEWAY_CAPABILITIES" > "${WORK_DIR}/gateway-capabilities"

    local external_service_info=""
    local external_log_info=""
    if [ -n "$EXT_PROC_EXTERNAL_PID" ]; then
        external_service_info="  - ext_proc_external: PID ${EXT_PROC_EXTERNAL_PID}, http://127.0.0.1:${EXT_PROC_EXTERNAL_ADMIN_PORT}"
        external_log_info="  - ${LOG_DIR}/ext_proc_external_processor.log"
    fi
    
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
${external_service_info}

Logs:
  - ${LOG_DIR}/test_server.log
  - ${LOG_DIR}/controller.log
  - ${LOG_DIR}/gateway.log
  - ${LOG_DIR}/edgion_access.log
${external_log_info}

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
        --ext-proc-external-bin)
            EXT_PROC_EXTERNAL_BIN="$2"
            shift 2
            ;;
        --ext-proc-external-grpc-port)
            EXT_PROC_EXTERNAL_GRPC_PORT="$2"
            shift 2
            ;;
        --ext-proc-external-admin-port)
            EXT_PROC_EXTERNAL_ADMIN_PORT="$2"
            shift 2
            ;;
        --ext-proc-external-tls-port)
            EXT_PROC_EXTERNAL_TLS_PORT="$2"
            shift 2
            ;;
        --ext-proc-external-mtls-port)
            EXT_PROC_EXTERNAL_MTLS_PORT="$2"
            shift 2
            ;;
        --ext-proc-external-config-template)
            EXT_PROC_EXTERNAL_CONFIG_TEMPLATE="$2"
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

    local external_arg_count=0
    [ -n "$EXT_PROC_EXTERNAL_BIN" ] && external_arg_count=$((external_arg_count + 1))
    [ -n "$EXT_PROC_EXTERNAL_GRPC_PORT" ] && external_arg_count=$((external_arg_count + 1))
    [ -n "$EXT_PROC_EXTERNAL_ADMIN_PORT" ] && external_arg_count=$((external_arg_count + 1))
    [ -n "$EXT_PROC_EXTERNAL_TLS_PORT" ] && external_arg_count=$((external_arg_count + 1))
    [ -n "$EXT_PROC_EXTERNAL_MTLS_PORT" ] && external_arg_count=$((external_arg_count + 1))
    [ -n "$EXT_PROC_EXTERNAL_CONFIG_TEMPLATE" ] && external_arg_count=$((external_arg_count + 1))
    if [ "$external_arg_count" -ne 0 ] && [ "$external_arg_count" -ne 6 ]; then
        log_error "All external ExtProc fixture arguments must be supplied together"
        exit 2
    fi
    if [ "$external_arg_count" -eq 6 ]; then
        case "$EXT_PROC_EXTERNAL_BIN" in
            /*) ;;
            *) log_error "External ExtProc fixture binary path must be absolute"; exit 2 ;;
        esac
        if [ "$EXT_PROC_EXTERNAL_BIN" != "${BIN_DIR}/ext_proc_external_processor" ]; then
            log_error "External ExtProc fixture binary must use the managed integration artifact path"
            exit 2
        fi
        case "$EXT_PROC_EXTERNAL_CONFIG_TEMPLATE" in
            /*) ;;
            *) log_error "External ExtProc config template path must be absolute"; exit 2 ;;
        esac
        if [ ! -x "$EXT_PROC_EXTERNAL_BIN" ]; then
            log_error "External ExtProc fixture binary is missing or not executable: ${EXT_PROC_EXTERNAL_BIN}"
            exit 1
        fi
        if [ ! -f "$EXT_PROC_EXTERNAL_CONFIG_TEMPLATE" ]; then
            log_error "External ExtProc config template is missing: ${EXT_PROC_EXTERNAL_CONFIG_TEMPLATE}"
            exit 1
        fi
        validate_port "$EXT_PROC_EXTERNAL_GRPC_PORT" "External ExtProc gRPC port" || exit 2
        validate_port "$EXT_PROC_EXTERNAL_ADMIN_PORT" "External ExtProc admin port" || exit 2
        validate_port "$EXT_PROC_EXTERNAL_TLS_PORT" "External ExtProc TLS gRPC port" || exit 2
        validate_port "$EXT_PROC_EXTERNAL_MTLS_PORT" "External ExtProc mTLS gRPC port" || exit 2
        local external_unique_ports
        external_unique_ports=$(printf '%s\n' "$EXT_PROC_EXTERNAL_GRPC_PORT" "$EXT_PROC_EXTERNAL_ADMIN_PORT" \
            "$EXT_PROC_EXTERNAL_TLS_PORT" "$EXT_PROC_EXTERNAL_MTLS_PORT" | sort -u | wc -l | tr -d ' ')
        if [ "$external_unique_ports" -ne 4 ]; then
            log_error "External ExtProc fixture ports must all be different"
            exit 2
        fi
    fi
    
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
    load_gateway_capabilities
    validate_capability_registry
    
    # : Workdirectory
    log_section "Workdirectory"
    mkdir -p "$LOG_DIR" "$PID_DIR" "$CONFIG_DIR" "$GENERATED_SECRET_DIR"
    log_success "Workdirectorycompleted: $WORK_DIR"

    start_external_ext_proc
    
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

    # conf_sync channel: mTLS, no injection.
    #
    # The checked-in config/edgion-*.yaml ALREADY carry the mTLS block with work-dir
    # relative cert paths, so the default path only has to make the certs exist at
    # <WORK_DIR>/certs/grpc/primary/ — there is nothing to rewrite. The configs are
    # still copied into the work dir because downstream scripts (reconnect / stress /
    # restart tests) restart the Controller from that copy.
    #
    # EDGION_CONF_SYNC_PLAINTEXT=1 is the ONE rewrite left here, and it exists solely
    # to drive run_conf_sync_skip_tls_metric_test.sh, whose subject is skip_tls.
    mkdir -p "${CONFIG_DIR}"
    cp "${PROJECT_ROOT}/config/edgion-controller.yaml" "${CONFIG_DIR}/edgion-controller.yaml"
    cp "${PROJECT_ROOT}/config/edgion-gateway.yaml" "${CONFIG_DIR}/edgion-gateway.yaml"
    CONTROLLER_CONFIG="${CONFIG_DIR}/edgion-controller.yaml"
    GATEWAY_CONFIG="${CONFIG_DIR}/edgion-gateway.yaml"
    if [ "${EDGION_TEST_DOWNSTREAM_TLS_OFFLOAD:-0}" = "1" ]; then
        awk '
            /^server:$/ {
                print
                print "  downstream_tls_offload_threadpools: 1"
                print "  downstream_tls_offload_thread_per_pool: 1"
                next
            }
            { print }
        ' "${GATEWAY_CONFIG}" > "${GATEWAY_CONFIG}.tmp" && mv "${GATEWAY_CONFIG}.tmp" "${GATEWAY_CONFIG}"
    fi
    # The L4 backpressure suite asserts the canonical TCP close diagnostic.
    # Enable only the copied integration config, preserving its logs/tcp.log sink.
    awk '
        /^tcp_log:/ { in_tcp = 1; print; next }
        /^[^[:space:]#]/ { in_tcp = 0 }
        in_tcp && /^  enabled:/ { print "  enabled: true"; next }
        { print }
    ' "${GATEWAY_CONFIG}" > "${GATEWAY_CONFIG}.tmp" && mv "${GATEWAY_CONFIG}.tmp" "${GATEWAY_CONFIG}"
    if grep -Eq '^[[:space:]]*metrics:' "${GATEWAY_CONFIG}"; then
        log_error "Integration metrics override requires a Gateway config without an existing metrics section"
        exit 1
    fi
    printf '\nmetrics:\n  backend_requests:\n    labels:\n      backend: true\n' >> "${GATEWAY_CONFIG}"
    log_info "Enabled backend identity metrics for integration assertions"
    if [ "${EDGION_TEST_ENABLE_GENAI_METRICS:-0}" = "1" ]; then
        printf '  genai_tokens:\n    enabled: true\n' >> "${GATEWAY_CONFIG}"
        log_info "Enabled GenAI token metrics in the integration workdir config"
    fi
    if [ "${EDGION_TEST_ENABLE_GEOIP:-0}" = "1" ]; then
        if grep -Eq '^  geoip:' "${GATEWAY_CONFIG}"; then
            log_error "Integration geoip override requires a Gateway config without an existing geoip section"
            exit 1
        fi
        if ! grep -Eq '^gateway:$' "${GATEWAY_CONFIG}"; then
            log_error "Integration geoip override requires a top-level 'gateway:' key to nest under"
            exit 1
        fi
        # `geoip` is a field of `GatewayConfig` with no `#[serde(flatten)]`, so it
        # MUST nest under the existing top-level `gateway:` key — appending a
        # second top-level `geoip:` key is silently ignored (`#[serde(default)]`
        # on the `gateway` field just defaults the whole nested struct, and a
        # gateway process never even calls `set_geoip_db_loaded`).
        # db_path must be absolute: a relative path resolves against --work-dir.
        # update_url is left unset so no updater spawns in the shared topology.
        geoip_block=$(printf '  geoip:\n    db_path: %s' \
            "${PROJECT_ROOT}/edgion-tests/geoip/edgion-test-city.mmdb")
        GEOIP_BLOCK="${geoip_block}" awk '
            { print }
            /^gateway:$/ && !done { print ENVIRON["GEOIP_BLOCK"]; done=1 }
        ' "${GATEWAY_CONFIG}" > "${GATEWAY_CONFIG}.tmp" && mv "${GATEWAY_CONFIG}.tmp" "${GATEWAY_CONFIG}"
        log_info "Enabled GeoIP (City-tier test fixture) in the integration workdir config"
    fi
    if [ "${EDGION_CONF_SYNC_PLAINTEXT:-0}" = "1" ]; then
        conf_sync_replace_with_skip_tls "${CONTROLLER_CONFIG}" "${GATEWAY_CONFIG}"
        CONF_SYNC_SCHEME="http"
        echo -e "${YELLOW}conf_sync: PLAINTEXT mode (EDGION_CONF_SYNC_PLAINTEXT=1)${NC}"
    else
        conf_sync_distribute_certs "${WORK_DIR}"
        CONF_SYNC_SCHEME="https"
        echo -e "${GREEN}conf_sync: mTLS mode (default, from the checked-in config)${NC}"
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

    # Full-only generated config is intentionally outside the recursive suite
    # loader. Apply it after ordinary resources and before generated Secrets.
    render_and_load_external_ext_proc_config

    # : Load generated Secret config（ API, override templates in conf/）
    load_generated_secrets
    
    # : Start gateway
    start_gateway
    
    # : verifyresourcesync
    verify_sync
    
    # info
    save_info

    # The published work directory now owns cleanup. Do not let this startup
    # shell's EXIT trap terminate a successfully handed-off fixture.
    EXT_PROC_EXTERNAL_STARTUP_OWNED=false
    trap - EXIT
    
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
