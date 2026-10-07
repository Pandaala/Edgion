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
SHELL_COMPAT="${UTILS_DIR}/shell_compat.sh"
# shellcheck source=../utils/shell_compat.sh
source "$SHELL_COMPAT"

# Whether to cleanup at end
DO_CLEANUP=true

# Whether to run full tests including slow tests (default: false for faster iteration)
FULL_TEST=false
AI_CONFORMANCE=false

# Required open files limit for integration run.
REQUIRED_NOFILE=65535

# Report file path (will be set after WORK_DIR is determined)
REPORT_FILE=""
REPORT_STARTED_AT="$(date '+%Y-%m-%d %H:%M:%S')"
REPORT_ENTRIES=()
REPORT_FINALIZED=false
REPORT_PHASE="argument parsing"

# Test counters
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0
SKIPPED_TESTS=0
TIMED_OUT_TESTS=0

# LDAP integration dependency (OpenLDAP via Docker Compose)
LDAP_COMPOSE_DIR=""
LDAP_COMPOSE_FILE=""
LDAP_SERVICE_REQUIRED=false
LDAP_SERVICE_STARTED=false

# LDAP TLS integration dependency (OpenLDAP with TLS via Docker Compose)
# Used by EdgionPlugins/LdapAuthTls.
LDAP_TLS_COMPOSE_DIR=""
LDAP_TLS_COMPOSE_FILE=""
LDAP_TLS_SERVICE_REQUIRED=false
LDAP_TLS_SERVICE_STARTED=false

# Redis integration dependency (used by EdgionPlugins/Dsl, EdgionPlugins/RateLimitRedis,
# and any LinkSys Redis test driven by the main runner). The bundled docker-compose
# matches edgion-tests/integration/conf/LinkSys/Redis (plaintext, TLS, and mTLS on
# loopback ports 16379-16381, password edgion-test-pwd). When the runner starts
# Redis itself, cleanup() stops it; externally managed services are left untouched.
REDIS_COMPOSE_DIR=""
REDIS_COMPOSE_FILE=""
REDIS_SERVICE_REQUIRED=false
REDIS_TLS_SERVICE_REQUIRED=false
REDIS_SERVICE_STARTED=false
REDIS_STARTED_SERVICES=()
REDIS_HOST="127.0.0.1"
REDIS_PORT=16379
REDIS_TLS_PORT=16380
REDIS_MTLS_PORT=16381
REDIS_PASSWORD="edgion-test-pwd"
REDIS_CONTAINER_NAME="edgion-test-redis"
REDIS_TLS_CONTAINER_NAME="edgion-test-redis-tls"
REDIS_MTLS_CONTAINER_NAME="edgion-test-redis-mtls"
REDIS_NATIVE_CA_FILE=""

CONTROLLER_GRPC_PORT=12151

EXT_PROC_EXTERNAL_GRPC_PORT=30044
EXT_PROC_EXTERNAL_ADMIN_PORT=30045
EXT_PROC_EXTERNAL_TLS_PORT=30046
EXT_PROC_EXTERNAL_MTLS_PORT=30047
EXT_PROC_EXTERNAL_ADMIN_URL="http://127.0.0.1:${EXT_PROC_EXTERNAL_ADMIN_PORT}"
EXT_PROC_EXTERNAL_FIXTURE_NAME="edgion-ext-proc-external"
EXT_PROC_EXTERNAL_PROTOCOL_VERSION="v1"
EXT_PROC_EXTERNAL_GO_CONTROL_PLANE_VERSION="v1.37.0"

# Standalone scenarios use isolated ports and work directories, so they can run
# concurrently without touching the shared Controller/Gateway topology. Keep the
# default bounded because each job may start its own Controller and Gateway.
INTEGRATION_JOBS="${EDGION_INTEGRATION_JOBS:-4}"
ISOLATED_SCENARIO_PIDS=()
ISOLATED_SCENARIO_TIMEOUT_SECONDS="${EDGION_ISOLATED_SCENARIO_TIMEOUT_SECONDS:-900}"
ISOLATED_PHASE_TIMEOUT_SECONDS="${EDGION_ISOLATED_PHASE_TIMEOUT_SECONDS:-1800}"
DEADLINE_GRACE_SECONDS="${EDGION_DEADLINE_GRACE_SECONDS:-10}"
DEADLINE_RUNNER="${UTILS_DIR}/run_with_deadline.py"

# Gateway admin bearer token. The gateway admin API (incl. the /api/v1/testing/*
# access-log endpoints) is auth-guarded, so test_client's AccessLogClient must
# present this token. start_all_with_conf.sh defaults the same value, but it runs
# in a command-substitution subshell, so its export does not reach this process or
# the test_client children launched here — export it here so the token is shared
# across the start script, the bash admin helpers, and test_client. Honors an
# operator-provided value.
export EDGION_GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-integration-gateway-admin-token-0123456789}"
# This runner enables includeRawUri in its generated Gateway config and exposes
# the local access-log admin API to the shared ForwardView client suite.
export EDGION_TEST_ACCESS_LOG_RAW_URI=1

# Controller admin token (token-only model). The Controller authenticates the
# Admin API with a single pre-shared bearer token read from EDGION_ADMIN_TOKEN.
# CTRL_ADMIN_TOKEN is populated by ensure_ctrl_admin_token() from that same value
# (exported before Step 2, after the no-auth preflight).
CTRL_ADMIN_TOKEN=""

# Whether to skip cache residue detection after tests
SKIP_RESIDUE_CHECK="${SKIP_RESIDUE_CHECK:-false}"

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

log_warn() {
    echo -e "${YELLOW}[!]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

is_external_ext_proc_required() {
    local selected_suites="$1"
    if ! $FULL_TEST; then
        return 1
    fi

    # The resource-wide Gateway sweep includes the full-only TLS recovery scenario.
    if [ "$G_RESOURCE" = "Gateway" ] && [ -z "$G_ITEM" ]; then
        return 0
    fi

    # Resource selection controls which tests execute even when --suites
    # overrides config loading, so an EdgionPlugins sweep/exact target always
    # needs the fixture.
    if [ "$G_RESOURCE" = "EdgionPlugins" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "ExtProc" ]; then
            return 0
        fi
    fi

    # An untargeted run without an explicit suite list is the default sweep.
    if [ -z "$G_RESOURCE" ] && [ -z "$selected_suites" ]; then
        return 0
    fi

    case ",${selected_suites}," in
        *,EdgionPlugins/ExtProc,*) return 0 ;;
    esac
    return 1
}

external_ext_proc_binary_path() {
    if [ "${EDGION_COVERAGE:-0}" = "1" ]; then
        printf '%s\n' "${PROJECT_ROOT}/target/llvm-cov-target/debug/ext_proc_external_processor"
    else
        printf '%s\n' "${PROJECT_ROOT}/target/debug/ext_proc_external_processor"
    fi
}

validate_external_ext_proc_artifact() {
    local fixture_bin="$1"
    case "$fixture_bin" in
        /*) ;;
        *)
            log_error "External ExtProc fixture path must be absolute: ${fixture_bin}"
            return 1
            ;;
    esac
    if [ ! -x "$fixture_bin" ]; then
        log_error "External ExtProc fixture is missing or not executable: ${fixture_bin}"
        log_error "Run prepare.sh --full-test before using --no-prepare"
        return 1
    fi
}

validate_external_ext_proc_readiness() {
    local response
    if ! response=$(curl --max-time 2 -fsS "${EXT_PROC_EXTERNAL_ADMIN_URL}/readyz" 2>/dev/null); then
        log_error "External ExtProc fixture readiness endpoint is unavailable: ${EXT_PROC_EXTERNAL_ADMIN_URL}/readyz"
        return 1
    fi
    if ! printf '%s' "$response" | python3 -c '
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
'; then
        log_error "External ExtProc fixture returned an unexpected readiness identity"
        return 1
    fi
}

validate_reused_external_ext_proc() {
    local current_file="${PROJECT_ROOT}/tmp/integration_testing/.current"
    if [ ! -f "$current_file" ]; then
        log_error "Cannot reuse external ExtProc fixture: work-directory marker is missing"
        return 1
    fi
    local reused_work_dir
    reused_work_dir=$(cat "$current_file")
    case "$reused_work_dir" in
        "${PROJECT_ROOT}/tmp/integration_testing/"*) ;;
        *)
            log_error "Refusing unexpected integration work directory: ${reused_work_dir}"
            return 1
            ;;
    esac
    if [ ! -f "${reused_work_dir}/config/ext_proc_external.yaml" ]; then
        log_error "Existing work directory has no loaded external ExtProc configuration evidence"
        return 1
    fi
    local material_dir="${reused_work_dir}/ext-proc-external-tls"
    local required_material
    for required_material in \
        server-ca.crt \
        client-ca.crt \
        server-a.crt \
        server-a.key \
        server-b.crt \
        server-b.key \
        client-a.crt \
        client-a.key \
        client-b.crt \
        client-b.key \
        ext-proc-external-server-ca.yaml \
        ext-proc-external-client-initial.yaml \
        ext-proc-external-client-updated.yaml; do
        if [ ! -f "${material_dir}/${required_material}" ]; then
            log_error "Existing work directory is missing external ExtProc TLS material: ${required_material}"
            return 1
        fi
    done
    validate_external_ext_proc_readiness
}

# =============================================================================
# Controller admin authentication (token-only model)
# =============================================================================
# The Controller authenticates the Admin API with a single pre-shared bearer
# token read from EDGION_ADMIN_TOKEN (exported before Step 2). There is no login
# step anymore; ensure_ctrl_admin_token() simply adopts that same value as the
# Bearer credential for subsequent admin API calls.
ensure_ctrl_admin_token() {
    if [ -n "$CTRL_ADMIN_TOKEN" ]; then
        return 0
    fi
    if [ -z "${EDGION_ADMIN_TOKEN:-}" ]; then
        log_error "EDGION_ADMIN_TOKEN not set — cannot authenticate to controller admin API"
        return 1
    fi
    CTRL_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN}"
    log_info "Controller admin token ready (pre-shared)"
}

# =============================================================================
# Slow test management
# =============================================================================
# Slow tests list (skipped by default, run with --full-test)
SLOW_TESTS=(
    "HTTPRoute_Backend_Timeout"
    "HTTPRoute_Backend_PerTryTimeout"
    "HTTPRoute_Backend_HealthCheckTransition"
    "EdgionPlugins_AllEndpointStatus"
    "EdgionPlugins_LdapAuth"
    "EdgionPlugins_LdapAuthTls"
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

check_docker_environment_ready() {
    if ! $FULL_TEST; then
        return 0
    fi

    log_section "Full Test Check: Docker Environment"

    if ! command -v docker >/dev/null 2>&1; then
        log_error "docker command not found"
        return 1
    fi

    if ! docker info >/dev/null 2>&1; then
        log_error "docker daemon is not reachable"
        return 1
    fi

    if ! docker compose version >/dev/null 2>&1; then
        log_error "docker compose is not available"
        return 1
    fi

    log_success "Docker environment is ready"
    return 0
}

# Loopback aliases required by HTTPRoute/Backend/LB* tests on macOS.
# Linux routes 127.0.0.0/8 to lo by default; macOS needs explicit lo0 aliases or
# packets to 127.0.0.2 / 127.0.0.3 are silently dropped (test_server bound to
# 0.0.0.0 still won't see them). Aliases also do not survive reboots, so we
# detect missing ones every run.
REQUIRED_LO_ALIASES=("127.0.0.2" "127.0.0.3" "127.0.0.4" "127.0.0.5" "127.0.0.6" "127.0.0.7")

ensure_loopback_aliases() {
    local missing_output missing=() ip
    missing_output=$(edgion_missing_loopback_aliases "${REQUIRED_LO_ALIASES[@]}")
    if [ -z "$missing_output" ]; then
        return 0
    fi
    while IFS= read -r ip; do
        [ -n "$ip" ] && missing+=("$ip")
    done <<< "$missing_output"

    log_error "lo0 is missing required aliases on macOS: ${missing[*]}"
    echo ""
    echo "  HTTPRoute/Backend/LB* tests use 127.0.0.2 / 127.0.0.3 / 127.0.0.4 as"
    echo "  distinct LB endpoints; 127.0.0.5 / 127.0.0.6 / 127.0.0.7 are the GeoIP"
    echo "  synthetic source addresses (RU / US / HK in the test fixture) used by"
    echo "  the EdgionPlugins|TCPRoute|TLSRoute GeoRestriction suites. macOS does"
    echo "  not auto-route 127/8 to lo0 like Linux, so without these aliases"
    echo "  connections silently time out."
    echo ""
    echo "  Run once now (aliases reset on reboot):"
    for ip in "${missing[@]}"; do
        echo "    sudo ifconfig lo0 alias ${ip} up"
    done
    echo ""
    echo "  Or make them persistent with launchd:"
    echo "    sudo tee /Library/LaunchDaemons/com.edgion.lo-alias.plist >/dev/null <<'EOF'"
    echo "    <?xml version=\"1.0\" encoding=\"UTF-8\"?>"
    echo "    <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyLists-1.0.dtd\">"
    echo "    <plist version=\"1.0\"><dict>"
    echo "      <key>Label</key><string>com.edgion.lo-alias</string>"
    echo "      <key>ProgramArguments</key><array>"
    echo "        <string>/bin/sh</string><string>-c</string>"
    echo "        <string>$(printf '/sbin/ifconfig lo0 alias %s up; ' "${REQUIRED_LO_ALIASES[@]}")</string>"
    echo "      </array>"
    echo "      <key>RunAtLoad</key><true/>"
    echo "    </dict></plist>"
    echo "    EOF"
    echo "    sudo launchctl load /Library/LaunchDaemons/com.edgion.lo-alias.plist"
    echo ""
    return 1
}

ensure_nofile_limit() {
    local required="${1:-65535}"
    local soft hard

    soft=$(ulimit -Sn 2>/dev/null || true)
    hard=$(ulimit -Hn 2>/dev/null || true)

    if [ -z "$soft" ] || [ -z "$hard" ]; then
        log_error "Cannot read current ulimit -n (soft/hard). Please run in a shell that supports ulimit."
        return 1
    fi

    if [ "$soft" = "unlimited" ]; then
        log_success "ulimit -n is unlimited"
        return 0
    fi

    if [ "$soft" -ge "$required" ]; then
        log_success "ulimit -n is sufficient (current: $soft, required: $required)"
        return 0
    fi

    log_info "Current ulimit -n is too low (soft=$soft hard=$hard), trying to set soft limit to $required"
    if ! ulimit -Sn "$required" 2>/dev/null; then
        log_error "Failed to set ulimit -n to $required (current soft=$soft hard=$hard)"
        echo "Please increase your shell file descriptor limit and rerun:"
        echo "  ulimit -n $required"
        return 1
    fi

    soft=$(ulimit -Sn 2>/dev/null || true)
    if [ -z "$soft" ]; then
        log_error "ulimit -n is still below required value after setting (current: ${soft:-unknown}, required: $required)"
        return 1
    fi
    if [ "$soft" != "unlimited" ] && [ "$soft" -lt "$required" ]; then
        log_error "ulimit -n is still below required value after setting (current: ${soft:-unknown}, required: $required)"
        return 1
    fi

    log_success "ulimit -n set successfully (current: $soft)"
    return 0
}

# Verify the Gateway process owned by this work directory and retain the
# single-shared-Gateway invariant.
check_single_gateway_process() {
    local pid_file="${WORK_DIR}/pids/gateway.pid"
    if [ ! -f "$pid_file" ]; then
        log_error "Gateway PID file not found: ${pid_file}"
        return 1
    fi
    local pid
    pid=$(cat "$pid_file")
    if ! kill -0 "$pid" 2>/dev/null; then
        log_error "Gateway process from ${pid_file} is not running (pid=${pid})"
        return 1
    fi

    local capabilities_file="${WORK_DIR}/gateway-capabilities"
    if [ ! -f "$capabilities_file" ]; then
        log_error "Running Gateway capability snapshot not found: ${capabilities_file}"
        return 1
    fi
    local running_capabilities
    running_capabilities=$(cat "$capabilities_file")
    if [ "$running_capabilities" != "$EDGION_TEST_GATEWAY_CAPABILITIES" ]; then
        log_error "Running Gateway capabilities differ from the selected binary"
        return 1
    fi

    local live_pids count
    live_pids=$(edgion_live_pids_by_name edgion-gateway)
    count=$(printf '%s\n' "$live_pids" | edgion_count_nonempty_lines)
    if [ "$count" -ne 1 ] || [ "$live_pids" != "$pid" ]; then
        log_error "Expected only the integration Gateway pid=${pid}, found: ${live_pids:-none}"
        return 1
    fi
    log_success "Integration Gateway process verified (pid=${pid})"
    return 0
}

# =============================================================================
# Report functions
# =============================================================================
init_report() {
    local pending_report="$REPORT_FILE"
    local final_report="${WORK_DIR}/report.log"
    if [ -n "$pending_report" ] && [ "$pending_report" != "$final_report" ] && [ -f "$pending_report" ]; then
        mv -f "$pending_report" "$final_report"
    fi
    REPORT_FILE="$final_report"
    write_report "INCOMPLETE"
}

init_pending_report() {
    local report_root="${PROJECT_ROOT}/tmp/integration_testing"
    mkdir -p "$report_root"
    REPORT_FILE="${report_root}/report-pending-$(date +%Y%m%d_%H%M%S)-$$.log"
    write_report "INCOMPLETE" "$REPORT_FILE" "not-created"
}

report_test() {
    local name=$1
    local result=$2
    local duration=$3
    local detail="${4:-}"
    local entry
    
    # Arithmetic assignments always return success under `set -e`; standalone
    # increment commands couple control flow to the expression value.
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    if [ "$result" = "PASS" ]; then
        PASSED_TESTS=$((PASSED_TESTS + 1))
        entry="[PASS] ${name} (${duration}s)"
    elif [ "$result" = "SKIP" ]; then
        SKIPPED_TESTS=$((SKIPPED_TESTS + 1))
        entry="[SKIP] ${name} (${duration}s)"
    elif [ "$result" = "TIMEOUT" ]; then
        FAILED_TESTS=$((FAILED_TESTS + 1))
        TIMED_OUT_TESTS=$((TIMED_OUT_TESTS + 1))
        entry="[TIMEOUT] ${name} (${duration}s)"
    else
        FAILED_TESTS=$((FAILED_TESTS + 1))
        entry="[FAIL] ${name} (${duration}s)"
    fi
    if [ -n "$detail" ]; then
        entry="${entry} - ${detail}"
    fi
    REPORT_ENTRIES+=("$entry")

    # Keep an atomically refreshed incomplete report during the run. Entries
    # recorded before the shared stack creates WORK_DIR remain buffered and are
    # written by init_report once the canonical report path is known.
    if [ -n "$REPORT_FILE" ]; then
        write_report "INCOMPLETE"
    fi
}

write_report() {
    local status=$1
    local target_file="${2:-$REPORT_FILE}"
    local report_work_dir="${3:-${WORK_DIR:-not-created}}"
    local target_dir
    target_dir=$(dirname "$target_file")
    mkdir -p "$target_dir"
    local tmp_file
    tmp_file=$(mktemp "${target_file}.tmp.XXXXXX")
    local executed_tests=$((PASSED_TESTS + FAILED_TESTS))
    {
        echo "========================================"
        echo "Edgion Integration Test Report"
        echo "Started: ${REPORT_STARTED_AT}"
        echo "Updated: $(date '+%Y-%m-%d %H:%M:%S')"
        echo "Status: ${status}"
        echo "Work Dir: ${report_work_dir}"
        echo "Full Test: ${FULL_TEST}"
        echo "========================================"
        echo ""
        local entry
        for entry in "${REPORT_ENTRIES[@]-}"; do
            [ -n "$entry" ] && echo "$entry"
        done
        echo ""
        echo "========================================"
        echo "Summary"
        echo "========================================"
        echo "Total:  ${TOTAL_TESTS}"
        echo "Passed: ${PASSED_TESTS}"
        echo "Failed: ${FAILED_TESTS}"
        echo "Timed out: ${TIMED_OUT_TESTS}"
        echo "Skipped: ${SKIPPED_TESTS}"
        if [ $executed_tests -gt 0 ]; then
            local pass_rate=$((PASSED_TESTS * 100 / executed_tests))
            echo "Pass Rate: ${pass_rate}%"
        fi
        echo "========================================"
    } > "$tmp_file"
    mv -f "$tmp_file" "$target_file"
}

finalize_report() {
    local status=$1
    write_report "$status"

    # Also print to console
    echo ""
    echo "Report saved to: ${REPORT_FILE}"
    REPORT_FINALIZED=true
}

# =============================================================================
# Dynamic Scenarios function (scenario-driven replacement of run_dynamic_tests)
# =============================================================================
run_dynamic_scenarios() {
    local test_client="${EDGION_TEST_CLIENT_BIN}"

    log_section "🔄 Dynamic Scenarios"

    if [ ! -x "$test_client" ]; then
        log_error "test_client binary not found: $test_client"
        return 1
    fi

    local ids
    if ! ids=$("$test_client" --list-dynamic-scenarios); then
        log_error "Dynamic scenario enumeration failed: $test_client --list-dynamic-scenarios"
        return 1
    fi
    if [ -z "$ids" ]; then
        log_error "Dynamic scenario enumeration returned an empty registry"
        return 1
    fi

    local scenario
    for scenario in $ids; do
        if [ "$scenario" = "ExtProcTlsPolicyRecovery" ] && ! $FULL_TEST; then
            log_info "Skipping ${scenario}: independent Go processor requires --full-test"
            report_test "Dyn_${scenario}" "SKIP" "0"
            continue
        fi
        run_test "Dyn_${scenario}" \
            "${test_client} -g --dynamic-scenario ${scenario}" || test_failed=true
    done

    log_success "Dynamic scenarios finished"
}

# =============================================================================
# Isolated standalone scenarios
# =============================================================================
run_isolated_scenarios() {
    local phase_started_at
    phase_started_at=$(date +%s)
    local names=(
        "controller_auth"
        "gateway_admin_auth"
        "fips_policy"
        "geoip_db_states"
        "unknown_config_fields"
        "proxy_protocol_policy_lifecycle"
        "udp_process_shutdown"
        "oidc_egress"
    )
    local commands=(
        "${SCRIPT_DIR}/test_controller_no_auth.sh"
        "${SCRIPT_DIR}/test_gateway_admin_auth.sh"
        "${SCRIPT_DIR}/test_fips_policy.sh"
        "${SCRIPT_DIR}/test_geoip_db_states.sh"
        "${SCRIPT_DIR}/test_unknown_config_fields.sh"
        "${SCRIPT_DIR}/test_proxy_protocol_policy_lifecycle.sh"
        "${SCRIPT_DIR}/test_udp_process_shutdown.sh"
        "${SCRIPT_DIR}/test_oidc_egress.sh"
    )

    local proxy_scenarios_skipped=false
    if $FULL_TEST; then
        names+=("ldap_auth_egress")
        commands+=("${SCRIPT_DIR}/test_ldap_auth_egress.sh")
    fi
    local docker_probe_timeout=15
    local probe_now probe_elapsed probe_remaining
    probe_now=$(date +%s)
    probe_elapsed=$((probe_now - phase_started_at))
    probe_remaining=$((ISOLATED_PHASE_TIMEOUT_SECONDS - probe_elapsed))
    if [ "$probe_remaining" -lt "$docker_probe_timeout" ]; then
        docker_probe_timeout="$probe_remaining"
    fi
    if [ "$docker_probe_timeout" -gt 0 ] \
        && command -v docker >/dev/null 2>&1 \
        && python3 "$DEADLINE_RUNNER" --timeout-secs "$docker_probe_timeout" --grace-secs "$DEADLINE_GRACE_SECONDS" -- \
            docker info >/dev/null 2>&1; then
        if ! python3 "${UTILS_DIR}/check_isolated_ports.py" \
            --scenario proxy_protocol_inbound \
            --scenario proxy_protocol_chain; then
            report_test "Preflight_IsolatedDockerPorts" "FAIL" "0"
            return 1
        fi
        report_test "Preflight_IsolatedDockerPorts" "PASS" "0"
        names+=("proxy_protocol_inbound" "proxy_protocol_chain")
        commands+=(
            "${SCRIPT_DIR}/test_proxy_protocol_inbound.sh"
            "${SCRIPT_DIR}/test_proxy_protocol_chain.sh"
        )
    else
        log_warn "docker unavailable; skipping PROXY protocol suites"
        proxy_scenarios_skipped=true
    fi

    local log_dir="${PROJECT_ROOT}/tmp/integration_testing/isolated_$(date +%Y%m%d_%H%M%S)_$$"
    mkdir -p "$log_dir"
    log_section "Isolated scenarios (parallelism: ${INTEGRATION_JOBS})"
    log_info "Isolated scenario logs: ${log_dir}"

    local pids=()
    local logs=()
    local starts=()
    local states=()
    local index=0
    local active=0
    local failed=false

    wait_for_isolated_scenario() {
        local wait_index=$1
        local name="${names[$wait_index]}"
        local pid="${pids[$wait_index]}"
        local log_file="${logs[$wait_index]}"
        local duration

        if wait "$pid"; then
            duration=$(( $(date +%s) - ${starts[$wait_index]} ))
            log_success "${name} passed (${duration}s)"
            report_test "Isolated_${name}" "PASS" "$duration"
        else
            local rc=$?
            duration=$(( $(date +%s) - ${starts[$wait_index]} ))
            if [ "$rc" -eq 124 ]; then
                log_error "${name} timed out (${duration}s): ${log_file}"
                report_test "Isolated_${name}" "TIMEOUT" "$duration" "deadline exceeded; log=${log_file}"
            else
                log_error "${name} failed (rc=${rc}, ${duration}s): ${log_file}"
                report_test "Isolated_${name}" "FAIL" "$duration" "rc=${rc}; log=${log_file}"
            fi
            tail -20 "$log_file" 2>/dev/null || true
            failed=true
        fi
        states[$wait_index]="done"
        active=$((active - 1))
        ISOLATED_SCENARIO_PIDS[$wait_index]=""
    }

    reap_finished_isolated_scenarios() {
        local scan_index=0
        local reaped=false
        while [ "$scan_index" -lt "${#pids[@]}" ]; do
            if [ "${states[$scan_index]}" = "running" ] \
                && ! kill -0 "${pids[$scan_index]}" 2>/dev/null; then
                wait_for_isolated_scenario "$scan_index"
                reaped=true
            fi
            scan_index=$((scan_index + 1))
        done
        $reaped
    }

    while [ "$index" -lt "${#names[@]}" ]; do
        while [ "$active" -ge "$INTEGRATION_JOBS" ]; do
            if ! reap_finished_isolated_scenarios; then
                sleep 1
            fi
        done

        local now phase_elapsed phase_remaining scenario_timeout
        now=$(date +%s)
        phase_elapsed=$((now - phase_started_at))
        phase_remaining=$((ISOLATED_PHASE_TIMEOUT_SECONDS - phase_elapsed))
        if [ "$phase_remaining" -lt 1 ]; then
            while [ "$index" -lt "${#names[@]}" ]; do
                log_error "${names[$index]} not started: isolated phase deadline reached"
                report_test "Isolated_${names[$index]}" "TIMEOUT" "0" "phase deadline reached before start"
                failed=true
                index=$((index + 1))
            done
            break
        fi
        if [ "$phase_remaining" -lt "$ISOLATED_SCENARIO_TIMEOUT_SECONDS" ]; then
            scenario_timeout="$phase_remaining"
        else
            scenario_timeout="$ISOLATED_SCENARIO_TIMEOUT_SECONDS"
        fi

        local name="${names[$index]}"
        local log_file="${log_dir}/${name}.log"
        log_info "Starting isolated scenario: ${name}"
        python3 "$DEADLINE_RUNNER" \
            --timeout-secs "$scenario_timeout" \
            --grace-secs "$DEADLINE_GRACE_SECONDS" \
            -- "${commands[$index]}" > "$log_file" 2>&1 &
        pids+=("$!")
        ISOLATED_SCENARIO_PIDS+=("$!")
        logs+=("$log_file")
        starts+=("$(date +%s)")
        states+=("running")
        active=$((active + 1))
        index=$((index + 1))
    done

    while [ "$active" -gt 0 ]; do
        if ! reap_finished_isolated_scenarios; then
            sleep 1
        fi
    done

    unset -f reap_finished_isolated_scenarios
    unset -f wait_for_isolated_scenario
    if $proxy_scenarios_skipped; then
        report_test "Isolated_proxy_protocol_inbound" "SKIP" "0" "docker unavailable"
        report_test "Isolated_proxy_protocol_chain" "SKIP" "0" "docker unavailable"
    fi
    if $failed; then
        log_error "One or more isolated scenarios failed"
        local failure_report="${log_dir}/report.log"
        write_report "INCOMPLETE" "$failure_report" "$log_dir"
        log_info "Isolated failure report: ${failure_report}"
        return 1
    fi
    log_success "All isolated scenarios passed"
}

# =============================================================================
# Cleanup function
# =============================================================================
cleanup() {
    local exit_code=$?
    set +e

    if [ "$REPORT_FINALIZED" != "true" ] && [ -n "$REPORT_FILE" ]; then
        if [ "$exit_code" -ne 0 ]; then
            report_test "Run_Aborted" "FAIL" "0" "phase=${REPORT_PHASE}; exit=${exit_code}"
        fi
        write_report "INCOMPLETE"
        log_info "Incomplete report saved to: ${REPORT_FILE}"
    fi

    local isolated_pid
    for isolated_pid in "${ISOLATED_SCENARIO_PIDS[@]-}"; do
        if [ -n "$isolated_pid" ] && kill -0 "$isolated_pid" 2>/dev/null; then
            kill "$isolated_pid" 2>/dev/null || true
            wait "$isolated_pid" 2>/dev/null || true
        fi
    done
    ISOLATED_SCENARIO_PIDS=()

    if $DO_CLEANUP; then
        log_section "Cleanup: Stop all services"
        if $LDAP_SERVICE_STARTED; then
            log_info "Stopping OpenLDAP test service..."
            docker compose -f "${LDAP_COMPOSE_FILE}" down --timeout 5 >/dev/null 2>&1 || true
        fi
        if $LDAP_TLS_SERVICE_STARTED; then
            log_info "Stopping TLS OpenLDAP test service..."
            docker compose -f "${LDAP_TLS_COMPOSE_FILE}" down --timeout 5 >/dev/null 2>&1 || true
        fi
        if $REDIS_SERVICE_STARTED; then
            log_info "Stopping Redis test service..."
            docker compose -f "${REDIS_COMPOSE_FILE}" rm -f -s \
                "${REDIS_STARTED_SERVICES[@]}" >/dev/null 2>&1 || true
        fi
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    else
        if $LDAP_SERVICE_STARTED; then
            log_info "OpenLDAP test service still running (keep-alive mode): ${LDAP_COMPOSE_FILE}"
        fi
        if $LDAP_TLS_SERVICE_STARTED; then
            log_info "TLS OpenLDAP test service still running (keep-alive mode): ${LDAP_TLS_COMPOSE_FILE}"
        fi
        if $REDIS_SERVICE_STARTED; then
            log_info "Redis test service still running (keep-alive mode): ${REDIS_COMPOSE_FILE}"
        fi
    fi

    return "$exit_code"
}

is_ldap_service_required() {
    # LDAP integration tests are full-test only.
    if ! $FULL_TEST; then
        return 1
    fi

    # Any full run contains EdgionPlugins/LdapAuth.
    if [ -z "$G_RESOURCE" ]; then
        return 0
    fi

    # Resource/item targeting.
    if [ "$G_RESOURCE" = "EdgionPlugins" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "LdapAuth" ]; then
            return 0
        fi
    fi

    # Explicit suites override.
    if [[ ",$1," == *",EdgionPlugins/LdapAuth,"* ]]; then
        return 0
    fi

    return 1
}

start_ldap_service_if_needed() {
    if ! $LDAP_SERVICE_REQUIRED; then
        return 0
    fi

    LDAP_COMPOSE_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/ldap-openldap"
    LDAP_COMPOSE_FILE="${LDAP_COMPOSE_DIR}/docker-compose.yml"

    if ! command -v docker >/dev/null 2>&1; then
        log_error "docker not found, but LdapAuth test requires OpenLDAP service"
        return 1
    fi

    if [ ! -f "${LDAP_COMPOSE_FILE}" ]; then
        log_error "LDAP compose file not found: ${LDAP_COMPOSE_FILE}"
        return 1
    fi

    log_section "LDAP dependency: start OpenLDAP"
    local compose_output=""
    compose_output=$(docker compose -f "${LDAP_COMPOSE_FILE}" up -d 2>&1) || {
        log_error "Failed to start OpenLDAP via docker compose"
        echo "$compose_output"
        return 1
    }

    local container_id=""
    container_id=$(docker compose -f "${LDAP_COMPOSE_FILE}" ps -q openldap 2>/dev/null || true)
    if [ -z "$container_id" ]; then
        log_error "OpenLDAP container was not created"
        return 1
    fi

    local elapsed=0
    local timeout=60
    while [ $elapsed -lt $timeout ]; do
        local health=""
        local state=""
        health=$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{end}}' "$container_id" 2>/dev/null || true)
        state=$(docker inspect -f '{{.State.Status}}' "$container_id" 2>/dev/null || true)

        if [ "$health" = "healthy" ] || { [ -z "$health" ] && [ "$state" = "running" ]; }; then
            LDAP_SERVICE_STARTED=true
            log_success "OpenLDAP is ready"
            break
        fi

        if [ "$state" = "exited" ] || [ "$state" = "dead" ]; then
            log_error "OpenLDAP container is not running (state=${state})"
            docker compose -f "${LDAP_COMPOSE_FILE}" logs --no-color --tail=80 || true
            return 1
        fi

        sleep 1
        elapsed=$((elapsed + 1))
    done

    if ! $LDAP_SERVICE_STARTED; then
        log_error "OpenLDAP did not become ready within ${timeout}s"
        docker compose -f "${LDAP_COMPOSE_FILE}" logs --no-color --tail=80 || true
        return 1
    fi

    local seed_file="${LDAP_COMPOSE_DIR}/bootstrap/10-users.ldif"
    if [ ! -f "$seed_file" ]; then
        log_error "LDAP seed file not found: $seed_file"
        return 1
    fi

    if docker exec edgion-test-openldap ldapsearch -x -H ldap://127.0.0.1:389 \
        -D cn=admin,dc=example,dc=org -w admin \
        -b ou=people,dc=example,dc=org '(uid=alice)' 2>/dev/null | grep -q "uid: alice"; then
        log_info "OpenLDAP seed data already present"
        return 0
    fi

    log_info "Seeding OpenLDAP test data..."
    if ! docker exec -i edgion-test-openldap ldapadd -x -H ldap://127.0.0.1:389 \
        -D cn=admin,dc=example,dc=org -w admin < "$seed_file" >/dev/null 2>&1; then
        log_error "Failed to seed OpenLDAP test data"
        docker compose -f "${LDAP_COMPOSE_FILE}" logs --no-color --tail=80 || true
        return 1
    fi

    if ! docker exec edgion-test-openldap ldapsearch -x -H ldap://127.0.0.1:389 \
        -D cn=admin,dc=example,dc=org -w admin \
        -b ou=people,dc=example,dc=org '(uid=alice)' 2>/dev/null | grep -q "uid: alice"; then
        log_error "OpenLDAP seed verification failed (alice not found)"
        return 1
    fi

    log_success "OpenLDAP test data seeded"
    return 0
}

# =============================================================================
# LDAP TLS integration dependency (OpenLDAP with TLS/mTLS via Docker Compose)
# =============================================================================
# Triggered by EdgionPlugins/LdapAuthTls or a full sweep with --full-test.
is_ldap_tls_service_required() {
    local explicit_suites="$1"

    # TLS LDAP tests are full-test only.
    if ! $FULL_TEST; then
        return 1
    fi

    # Any full run contains EdgionPlugins/LdapAuthTls.
    if [ -z "$G_RESOURCE" ]; then
        return 0
    fi

    # Resource/item targeting.
    if [ "$G_RESOURCE" = "EdgionPlugins" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "LdapAuthTls" ]; then
            return 0
        fi
    fi

    # Explicit suites override.
    if [[ ",$explicit_suites," == *",EdgionPlugins/LdapAuthTls,"* ]]; then
        return 0
    fi

    return 1
}

start_ldap_tls_service_if_needed() {
    if ! $LDAP_TLS_SERVICE_REQUIRED; then
        return 0
    fi

    LDAP_TLS_COMPOSE_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/ldap-openldap-tls"
    LDAP_TLS_COMPOSE_FILE="${LDAP_TLS_COMPOSE_DIR}/docker-compose.yml"

    if ! command -v docker >/dev/null 2>&1; then
        log_error "docker not found, but LdapAuthTls test requires TLS OpenLDAP service"
        return 1
    fi

    if [ ! -f "${LDAP_TLS_COMPOSE_FILE}" ]; then
        log_error "LDAP TLS compose file not found: ${LDAP_TLS_COMPOSE_FILE}"
        return 1
    fi

    local cert_script="${LDAP_TLS_COMPOSE_DIR}/make-test-certs.sh"
    if [ ! -f "$cert_script" ] || ! bash "$cert_script"; then
        log_error "Failed to prepare LDAP TLS test certificates"
        return 1
    fi

    log_section "LDAP TLS dependency: start TLS OpenLDAP"
    local compose_output=""
    # Recreate the containers so a previously missing bind-mounted certificate
    # (which Docker materializes as a directory) cannot poison later runs after
    # the certificate generator repairs the host path.
    compose_output=$(docker compose -f "${LDAP_TLS_COMPOSE_FILE}" up -d --force-recreate 2>&1) || {
        log_error "Failed to start TLS OpenLDAP via docker compose"
        echo "$compose_output"
        return 1
    }

    # Wait for the LDAPS container (openldap-tls) to become ready.
    local container_id=""
    container_id=$(docker compose -f "${LDAP_TLS_COMPOSE_FILE}" ps -q openldap-tls 2>/dev/null || true)
    if [ -z "$container_id" ]; then
        log_error "TLS OpenLDAP container (openldap-tls) was not created"
        return 1
    fi

    local elapsed=0
    local timeout=60
    while [ $elapsed -lt $timeout ]; do
        local health=""
        local state=""
        health=$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{end}}' "$container_id" 2>/dev/null || true)
        state=$(docker inspect -f '{{.State.Status}}' "$container_id" 2>/dev/null || true)

        if [ "$health" = "healthy" ] || { [ -z "$health" ] && [ "$state" = "running" ]; }; then
            LDAP_TLS_SERVICE_STARTED=true
            log_success "TLS OpenLDAP is ready"
            break
        fi

        if [ "$state" = "exited" ] || [ "$state" = "dead" ]; then
            log_error "TLS OpenLDAP container is not running (state=${state})"
            docker compose -f "${LDAP_TLS_COMPOSE_FILE}" logs --no-color --tail=80 || true
            return 1
        fi

        sleep 1
        elapsed=$((elapsed + 1))
    done

    if ! $LDAP_TLS_SERVICE_STARTED; then
        log_error "TLS OpenLDAP did not become ready within ${timeout}s"
        docker compose -f "${LDAP_TLS_COMPOSE_FILE}" logs --no-color --tail=80 || true
        return 1
    fi

    return 0
}

# =============================================================================
# Redis integration dependency
# =============================================================================
# Triggered by Redis-dependent plugin and AI Gateway suites, or the default
# full sweep. The runner first probes whether Redis is already reachable on
# 127.0.0.1:16379; if not, it tries to start the bundled docker-compose
# stack (matches edgion-tests/integration/conf/LinkSys/Redis); if Docker is unavailable
# the runner aborts with platform-specific install hints.
is_redis_service_required() {
    local explicit_suites="$1"

    # Default full sweep (no -r/-i) covers DSL + RateLimitRedis + LinkSys/Redis.
    if [ -z "$G_RESOURCE" ]; then
        return 0
    fi

    # The Gateway family also runs LinkSysDisabledTls in the dynamic phase.
    if [ "$G_RESOURCE" = "Gateway" ] && [ -z "$G_ITEM" ]; then
        return 0
    fi

    if [ "$G_RESOURCE" = "EdgionPlugins" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "Dsl" ] || [ "$G_ITEM" = "RateLimitRedis" ]; then
            return 0
        fi
    fi

    if [ "$G_RESOURCE" = "EdgionBackend" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "SingleProvider" ] || [ "$G_ITEM" = "CredentialExhaustion" ] || { [ "$G_ITEM" = "Responses" ] || [ "$G_ITEM" = "Messages" ]; }; then
            return 0
        fi
    fi

    if [ "$G_RESOURCE" = "LinkSys" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "Redis" ] || [ "$G_ITEM" = "RedisTls" ]; then
            return 0
        fi
    fi

    if [ "$G_RESOURCE" = "TCPRoute" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "StreamPlugins" ]; then
            return 0
        fi
    fi

    case ",${explicit_suites}," in
        *,EdgionPlugins/Dsl,*|*,EdgionPlugins/RateLimitRedis,*|*,EdgionBackend/SingleProvider,*|*,EdgionBackend/Responses,*|*,EdgionBackend/Messages,*|*,EdgionBackend/CredentialExhaustion,*|*,LinkSys/Redis,*|*,LinkSys/RedisTls,*|*,TCPRoute/StreamPlugins,*)
            return 0
            ;;
    esac

    return 1
}

is_redis_tls_service_required() {
    local explicit_suites="$1"

    if [ -z "$G_RESOURCE" ] && [ -z "$explicit_suites" ]; then
        return 0
    fi
    if [ "$G_RESOURCE" = "LinkSys" ] \
        && { [ -z "$G_ITEM" ] || [ "$G_ITEM" = "Redis" ] || [ "$G_ITEM" = "RedisTls" ]; }; then
        return 0
    fi
    case ",${explicit_suites}," in
        *,LinkSys/RedisTls,*) return 0 ;;
    esac

    return 1
}

# Probe whether Redis is reachable. Order of attempts:
#   1) host redis-cli (most accurate — verifies AUTH);
#   2) running edgion-test-redis container with `docker exec`;
#   3) bare TCP connect to host:port (port-open is treated as good enough —
#      a wrong-password Redis at this port would still produce non-PONG and
#      surface as a normal test failure, which is the same signal the operator
#      would have seen pre-change).
probe_redis_ready() {
    local host=$1 port=$2 password=$3

    if command -v redis-cli >/dev/null 2>&1; then
        if redis-cli -h "$host" -p "$port" -a "$password" --no-auth-warning ping 2>/dev/null \
            | grep -q '^PONG$'; then
            return 0
        fi
    fi

    if command -v docker >/dev/null 2>&1; then
        if docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$REDIS_CONTAINER_NAME"; then
            if docker exec "$REDIS_CONTAINER_NAME" \
                redis-cli -a "$password" ping 2>/dev/null | grep -q '^PONG$'; then
                return 0
            fi
        fi
    fi

    if (exec 3<>"/dev/tcp/${host}/${port}") 2>/dev/null; then
        exec 3<&- 3>&- 2>/dev/null || true
        return 0
    fi

    return 1
}

# Prove the live Redis TLS/mTLS server presents a certificate the current run
# will trust. Host certificate files are the only identity source: a docker
# exec against container-local /certs can succeed for a healthy leftover
# container even after make-test-certs.sh replaced the host material that
# Gateway Secrets and SSL_CERT_FILE will load.
probe_redis_tls_identity() {
    local host=$1 port=$2 require_client_cert=$3
    local certs_dir="${REDIS_COMPOSE_DIR:-${PROJECT_ROOT}/edgion-tests/integration/conf/Services/redis}/certs"
    local ca="${certs_dir}/ca.crt"
    local output
    local openssl_args

    [ -f "$ca" ] || return 1
    openssl_args=(
        s_client
        -connect "${host}:${port}"
        -CAfile "$ca"
        -verify_return_error
    )
    if [ "$require_client_cert" = "true" ]; then
        [ -f "${certs_dir}/client.crt" ] && [ -f "${certs_dir}/client.key" ] || return 1
        openssl_args+=(-cert "${certs_dir}/client.crt" -key "${certs_dir}/client.key")
    fi

    # LibreSSL can still exit 0 with -verify_return_error when the chain does
    # not verify. The Verify return code line is the fail-closed identity gate.
    output=$(printf 'Q\n' | openssl "${openssl_args[@]}" 2>&1) || return 1
    printf '%s\n' "$output" | grep -F -q 'Verify return code: 0 (ok)'
}

probe_redis_tls_ready() {
    local host=$1 port=$2 password=$3 require_client_cert=$4
    local certs_dir="${REDIS_COMPOSE_DIR:-${PROJECT_ROOT}/edgion-tests/integration/conf/Services/redis}/certs"
    local tls_args=(--tls --cacert "${certs_dir}/ca.crt")

    probe_redis_tls_identity "$host" "$port" "$require_client_cert" || return 1

    if ! command -v redis-cli >/dev/null 2>&1; then
        return 0
    fi
    if [ "$require_client_cert" = "true" ]; then
        tls_args+=(--cert "${certs_dir}/client.crt" --key "${certs_dir}/client.key")
    fi
    if redis-cli "${tls_args[@]}" -h "$host" -p "$port" -a "$password" \
        --no-auth-warning ping 2>/dev/null | grep -q '^PONG$'; then
        return 0
    fi

    # Identity already matched the current host material. A host redis-cli
    # without TLS support must not force a recreate.
    return 0
}

print_redis_install_hints() {
    cat <<EOF

Edgion integration tests require a Redis instance reachable at
  ${REDIS_HOST}:${REDIS_PORT}   plaintext
  ${REDIS_HOST}:${REDIS_TLS_PORT}   TLS with the generated test CA
  ${REDIS_HOST}:${REDIS_MTLS_PORT}  mutual TLS with the generated client identity
  password: ${REDIS_PASSWORD}

This matches the LinkSys fixture in edgion-tests/integration/conf/LinkSys/Redis
(consumed by EdgionPlugins/Dsl, EdgionPlugins/RateLimitRedis, and the
LinkSys Redis suite).

Recommended (any platform with Docker):
  cd ${REDIS_COMPOSE_DIR}
  docker compose up -d
  # the runner reuses it only when the live server still matches the current certificates

Manual install:
  macOS (Homebrew):
    brew install redis
    redis-server --port ${REDIS_PORT} --requirepass ${REDIS_PASSWORD} --daemonize yes
  Debian / Ubuntu:
    sudo apt-get install -y redis-server
    redis-server --port ${REDIS_PORT} --requirepass ${REDIS_PASSWORD} --daemonize yes
  RHEL / CentOS / Fedora:
    sudo dnf install -y redis      # or: sudo yum install -y redis
    redis-server --port ${REDIS_PORT} --requirepass ${REDIS_PASSWORD} --daemonize yes
  Windows (WSL2):
    use the Linux instructions inside your WSL distro, or run Docker Desktop:
    docker run -d --name ${REDIS_CONTAINER_NAME} -p ${REDIS_PORT}:6379 \\
        redis:7-alpine redis-server --requirepass ${REDIS_PASSWORD}

After Redis is reachable, re-run the same integration test command.

EOF
}

# Build the command line for the LinkSys/Redis assertions driver.
# Lifecycle deletion (test 10 — delete the redis-test LinkSys CRD and verify
# the runtime client is removed) is included by default. It is safe in the
# normal test order: LinkSys_Redis runs after every EdgionPlugins/Status/
# LinkSys_Status suite that depends on default/redis-test, and the driver
# re-applies the CRD at the end as a safety net.
linksys_redis_cmd() {
    echo "bash ${SCRIPT_DIR}/link_sys/redis_assertions.sh --include-lifecycle"
}

start_redis_service_if_needed() {
    if ! $REDIS_SERVICE_REQUIRED; then
        return 0
    fi

    REDIS_COMPOSE_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/redis"
    REDIS_COMPOSE_FILE="${REDIS_COMPOSE_DIR}/docker-compose.yaml"
    local cert_script="${REDIS_COMPOSE_DIR}/make-test-certs.sh"

    log_section "Redis dependency: ensure Redis is available"

    local plaintext_ready=false
    local tls_ready=true
    local mtls_ready=true
    probe_redis_ready "$REDIS_HOST" "$REDIS_PORT" "$REDIS_PASSWORD" && plaintext_ready=true

    if $REDIS_TLS_SERVICE_REQUIRED; then
        if [ ! -f "$cert_script" ] || ! command -v openssl >/dev/null 2>&1; then
            log_error "Redis TLS certificate preparation requires ${cert_script} and openssl"
            return 1
        fi
        if ! bash "$cert_script"; then
            log_error "Redis TLS certificate preparation failed"
            return 1
        fi

        # fred's scheme-only rediss:// path uses rustls-native-certs. Point its
        # documented file source at the ephemeral test CA so this isolated suite
        # exercises default trust without modifying the host trust store.
        REDIS_NATIVE_CA_FILE="${REDIS_COMPOSE_DIR}/certs/ca.crt"
        tls_ready=false
        mtls_ready=false
        probe_redis_tls_ready "$REDIS_HOST" "$REDIS_TLS_PORT" "$REDIS_PASSWORD" false && tls_ready=true
        probe_redis_tls_ready "$REDIS_HOST" "$REDIS_MTLS_PORT" "$REDIS_PASSWORD" true && mtls_ready=true
    fi

    if $plaintext_ready && $tls_ready && $mtls_ready; then
        log_success "Required Redis services are already reachable (reusing them)"
        return 0
    fi

    log_info "One or more Redis transports are unavailable — attempting to start bundled services"

    if ! command -v docker >/dev/null 2>&1 \
        || ! docker info >/dev/null 2>&1 \
        || ! docker compose version >/dev/null 2>&1; then
        log_error "Redis is required but not running, and Docker is unavailable on this host."
        print_redis_install_hints
        return 1
    fi

    if [ ! -f "${REDIS_COMPOSE_FILE}" ]; then
        log_error "Redis compose file not found: ${REDIS_COMPOSE_FILE}"
        return 1
    fi

    local services=()
    local plaintext_services=()
    local tls_services=()
    $plaintext_ready || plaintext_services+=(redis)
    $tls_ready || tls_services+=(redis-tls)
    $mtls_ready || tls_services+=(redis-mtls)
    [ ${#plaintext_services[@]} -eq 0 ] || services+=("${plaintext_services[@]}")
    [ ${#tls_services[@]} -eq 0 ] || services+=("${tls_services[@]}")

    local compose_output=""
    REDIS_STARTED_SERVICES=("${services[@]}")
    REDIS_SERVICE_STARTED=true
    # Redis loads TLS files at process start. `up -d` leaves a healthy leftover
    # container running with the previous in-memory certificates, so identity
    # mismatches must recreate the TLS/mTLS services.
    if [ ${#plaintext_services[@]} -gt 0 ]; then
        compose_output=$(docker compose -f "${REDIS_COMPOSE_FILE}" up -d \
            "${plaintext_services[@]}" 2>&1) || {
            log_error "Failed to start Redis via docker compose"
            echo "$compose_output"
            print_redis_install_hints
            return 1
        }
    fi
    if [ ${#tls_services[@]} -gt 0 ]; then
        compose_output=$(docker compose -f "${REDIS_COMPOSE_FILE}" up -d --force-recreate \
            "${tls_services[@]}" 2>&1) || {
            log_error "Failed to start Redis TLS via docker compose"
            echo "$compose_output"
            print_redis_install_hints
            return 1
        }
    fi

    local elapsed=0
    local timeout=30
    while [ $elapsed -lt $timeout ]; do
        if probe_redis_ready "$REDIS_HOST" "$REDIS_PORT" "$REDIS_PASSWORD" \
            && { ! $REDIS_TLS_SERVICE_REQUIRED \
                || { probe_redis_tls_ready "$REDIS_HOST" "$REDIS_TLS_PORT" "$REDIS_PASSWORD" false \
                    && probe_redis_tls_ready "$REDIS_HOST" "$REDIS_MTLS_PORT" "$REDIS_PASSWORD" true; }; }; then
            log_success "Required Redis services are ready (waited ${elapsed}s)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    log_error "Required Redis services did not become ready within ${timeout}s"
    docker compose -f "${REDIS_COMPOSE_FILE}" logs --no-color --tail=80 || true
    return 1
}

# =============================================================================
# Get server_id from controller
# =============================================================================
get_controller_server_id() {
    local controller_url="${1:-http://127.0.0.1:12101}"
    # Controller admin API requires authentication; obtain a token first.
    ensure_ctrl_admin_token || true
    local response=$(curl -s -H "Authorization: Bearer ${CTRL_ADMIN_TOKEN}" \
        "${controller_url}/api/v1/server-info" 2>/dev/null)
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
    local gateway_url="${1:-http://127.0.0.1:12001}"
    # Gateway admin API now requires a bearer token (see start_all_with_conf.sh).
    : "${EDGION_GATEWAY_ADMIN_TOKEN:=integration-gateway-admin-token-0123456789}"
    local response=$(curl -s -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" \
        "${gateway_url}/api/v1/server-info" 2>/dev/null)
    local server_id=$(echo "$response" | grep -o '"server_id":"[^"]*"' | cut -d'"' -f4)
    if [ -z "$server_id" ]; then
        server_id="unknown"
    fi
    echo "$server_id"
}

# =============================================================================
# Verify all `role="..."` label values in /metrics are in the expected whitelist.
# Defends against cardinality explosion from typos / new roles introduced unintentionally.
# Returns 0 only when the endpoint was read and all labels are valid.
# =============================================================================
verify_metrics_labels() {
    local endpoint=$1 component=$2
    local body curl_rc
    if body=$(curl -sf -m 5 "$endpoint" 2>/dev/null); then
        curl_rc=0
    else
        curl_rc=$?
    fi
    if [ $curl_rc -ne 0 ] || [ -z "$body" ]; then
        log_error "[$component] /metrics could not be read: $endpoint (curl rc=${curl_rc})"
        return 1
    fi

    # Extract all role="..." label values, dedup
    local roles
    roles=$(echo "$body" | grep -oE 'role="[^"]+"' | sort -u)

    if [ -z "$roles" ]; then
        log_info "[$component] /metrics has no role= labels (no federation metrics emitted yet — OK)"
        return 0
    fi

    local bad=0
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        case "$line" in
            'role="controller"')
                ;;
            *)
                log_error "[$component] /metrics has unexpected role label: $line (allowed: controller)"
                bad=1
                ;;
        esac
    done <<< "$roles"

    if [ $bad -ne 0 ]; then
        return 1
    fi
    log_info "[$component] /metrics role labels validated: $(echo "$roles" | tr '\n' ' ')"
    return 0
}

# =============================================================================
# Verify no resource in the fleet carries an unknown (typo'd) config key.
#
# Backed by `GET /api/v1/diagnostics/unknown-fields`; see
# docs/en/ops-guide/plugin-config-unknown-fields.md. This is a proof of
# ABSENCE: if any plugin's config key was dropped by a schema mismatch, every
# later assertion about that plugin is suspect, so a caller of this function
# is expected to hard-fail the run rather than continue on a false-green
# suite. See `test_unknown_config_fields.sh` for the matching proof of
# PRESENCE (a dedicated scenario that deliberately trips this check).
#
# Returns 0 only when the authenticated endpoint is reachable and total == 0.
# Infrastructure and authentication errors fail closed, just like a non-zero
# total, so a caller cannot mistake an unperformed validation for success. The
# function prints every
# "kind/namespace/name -> paths" line so the operator can act without
# digging further.
# =============================================================================
verify_no_unknown_config_fields() {
    local controller_url="${1:-http://127.0.0.1:12101}"
    if ! ensure_ctrl_admin_token || [ -z "$CTRL_ADMIN_TOKEN" ]; then
        log_error "Unknown-config-fields check failed -- controller admin token unavailable"
        return 1
    fi

    # Separate "could not reach the controller at all" (infra noise -- skip) from
    # "the controller answered, but not with 200" (a 401/403/404/500 means the
    # route, its authz classification, or the middleware chain regressed). The
    # earlier `curl -sf` form collapsed both into an empty body and passed, which
    # would have made this guard permanently, silently green after such a
    # regression -- defeating the only thing it exists to do.
    local response http_code body
    local curl_rc
    if response=$(curl -s -m 10 -w $'\n%{http_code}' -H "Authorization: Bearer ${CTRL_ADMIN_TOKEN}" \
        "${controller_url}/api/v1/diagnostics/unknown-fields" 2>/dev/null); then
        curl_rc=0
    else
        curl_rc=$?
    fi
    if [ $curl_rc -ne 0 ]; then
        log_error "Unknown-config-fields check failed -- diagnostics endpoint unreachable (curl rc=${curl_rc}): ${controller_url}"
        return 1
    fi
    http_code=$(echo "$response" | tail -1)
    body=$(echo "$response" | sed '$d')
    if [ "$http_code" != "200" ]; then
        log_error "Diagnostics endpoint answered HTTP ${http_code} (expected 200): ${controller_url}/api/v1/diagnostics/unknown-fields"
        log_error "  The controller is up, so this is a route/authz/middleware regression, not infrastructure noise."
        return 1
    fi
    if [ -z "$body" ]; then
        log_error "Diagnostics endpoint returned HTTP 200 with an empty body -- treating as a regression, not infra noise"
        return 1
    fi

    local report parse_rc
    if report=$(printf '%s' "$body" | python3 -c '
import json, sys
try:
    doc = json.load(sys.stdin)
except Exception:
    sys.exit(1)
data = doc.get("data") if isinstance(doc, dict) else None
if not isinstance(doc, dict) or doc.get("success") is not True or not isinstance(data, dict):
    sys.exit(1)
total = data.get("total")
truncated = data.get("truncated")
items = data.get("items")
if isinstance(total, bool) or not isinstance(total, int) or total < 0:
    sys.exit(1)
if not isinstance(truncated, bool) or not isinstance(items, list):
    sys.exit(1)
if (truncated and total <= len(items)) or (not truncated and total != len(items)):
    sys.exit(1)
print(f"TOTAL={total}")
for item in items:
    if not isinstance(item, dict):
        sys.exit(1)
    kind = item.get("kind")
    ns = item.get("namespace")
    name = item.get("name")
    paths = item.get("paths")
    if not isinstance(kind, str) or not kind or not isinstance(ns, str):
        sys.exit(1)
    if not isinstance(name, str) or not name or not isinstance(paths, list) or not paths:
        sys.exit(1)
    if any(not isinstance(path, str) or not path for path in paths):
        sys.exit(1)
    paths = ", ".join(paths)
    print(f"{kind}/{ns}/{name} -> {paths}")
' 2>/dev/null); then
        parse_rc=0
    else
        parse_rc=$?
    fi

    if [ $parse_rc -ne 0 ] || [ -z "$report" ]; then
        # A 200 response whose body is not the documented shape means the
        # endpoint's contract changed. Failing here is the point: silently
        # skipping would hide exactly the regression this guard must catch.
        log_error "Diagnostics endpoint returned HTTP 200 but an unparseable/unexpected body -- contract regression"
        return 1
    fi

    local total_line total
    total_line=$(echo "$report" | head -1)
    total=${total_line#TOTAL=}

    case "$total" in
        ''|*[!0-9]*)
            log_error "Diagnostics endpoint returned an invalid total: ${total:-empty}"
            return 1
            ;;
    esac

    if [ "${total:-0}" -gt 0 ] 2>/dev/null; then
        log_error "Unknown config fields detected on ${total} resource(s):"
        echo "$report" | tail -n +2 | while IFS= read -r line; do
            [ -z "$line" ] && continue
            log_error "  $line"
        done
        return 1
    fi

    log_success "Unknown-config-fields check passed (total: 0)"
    return 0
}

# =============================================================================
# Trigger reload via Admin API
# =============================================================================
trigger_reload() {
    local controller_url="${1:-http://127.0.0.1:12101}"
    log_info "Triggering reload via Admin API..."
    # Controller admin API requires authentication.
    ensure_ctrl_admin_token || true
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${CTRL_ADMIN_TOKEN}" \
        "${controller_url}/api/v1/reload" 2>/dev/null)
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
    echo "  --ai-conformance       Local native AI matrix (requires Messages target and both SDK interpreters)"
    echo "  --full-test            Run all tests including slow tests (timeout, etc.)"
    echo "  --jobs <N>             Max parallel isolated scenarios (default: 4)"
    echo "  --isolated-timeout-secs <N>       Per-isolated-scenario deadline (default: 900)"
    echo "  --isolated-phase-timeout-secs <N> Whole isolated phase deadline (default: 1800)"
    echo "  --controller-grpc-port <port>  Override controller/gateway gRPC sync port (default: 12151)"
    echo "  --suites <list>        Specify test suites to load (comma separated)"
    echo "  --with-reload          Run tests twice with reload in between (verify server_id changes)"
    echo "  --skip-residue-check   Skip cache residue detection after tests (also set via SKIP_RESIDUE_CHECK=true)"
    echo "                         Set RESIDUE_MODE=warn env to log leaks without failing the build (default: fail — leaks are errors)"
    echo "  -h, --help             Show help"
    echo ""
    echo "Examples:"
    echo "  $0                                  # Run all integration tests"
    echo "  $0 -r HTTPRoute                     # Run all HTTPRoute tests"
    echo "  $0 -r HTTPRoute -i Match            # Run HTTPRoute/Match test"
    echo "  $0 -r HTTPRoute -i Backend          # Run HTTPRoute/Backend test"
    echo "  $0 --no-prepare -r HTTPRoute        # Skip build, run HTTPRoute test"
    echo "  $0 --with-reload                    # Run all tests, reload, verify server_id changed, run again"
    echo "  $0 --controller-grpc-port 40051    # Run integration with alternate controller/gateway gRPC port"
}

# =============================================================================
# Main function
# =============================================================================

# Global variables for test context (set by main, used by run_all_tests)
G_RESOURCE=""
G_ITEM=""

# =============================================================================
# Run all tests function (can be called multiple times)
# =============================================================================
run_credential_reservation_redis_test() {
    local test_bin="${EDGION_CREDENTIAL_RESERVATION_TEST_BIN:-}"
    if [ -z "$test_bin" ] || [ ! -x "$test_bin" ]; then
        log_error "Credential reservation Redis test executable is unavailable: ${test_bin:-not configured}"
        report_test "CredentialReservationRedis" "FAIL" "0" "precompiled executable unavailable"
        return 1
    fi
    run_test "CredentialReservationRedis" "${test_bin} --ignored --nocapture"
}

run_token_bucket_partial_refill_redis_test() {
    local test_bin="${EDGION_TOKEN_BUCKET_PARTIAL_REFILL_TEST_BIN:-}"
    if [ -z "$test_bin" ] || [ ! -x "$test_bin" ]; then
        log_error "Token-bucket partial-refill Redis test executable is unavailable: ${test_bin:-not configured}"
        report_test "TokenBucketPartialRefillRedis" "FAIL" "0" "precompiled executable unavailable"
        return 1
    fi
    run_test "TokenBucketPartialRefillRedis" "${test_bin} --ignored --nocapture"
}

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
            EdgionBackend)
                if [ -z "$G_ITEM" ]; then
                    run_test "EdgionBackend_SingleProvider" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i SingleProvider" || test_failed=true
                    run_test "EdgionBackend_Responses" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i Responses" || test_failed=true
        run_test "EdgionBackend_Messages" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i Messages" || test_failed=true
        run_test "EdgionBackend_AnthropicProvider" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i AnthropicProvider" || test_failed=true
                    run_test "EdgionBackend_Resilience" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i Resilience" || test_failed=true
                    run_test "EdgionBackend_CredentialExhaustion" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i CredentialExhaustion" || test_failed=true
                    run_credential_reservation_redis_test || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "EdgionBackend_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i ${G_ITEM}" || test_failed=true
                    if [ "$G_ITEM" = "CredentialExhaustion" ]; then
                        run_credential_reservation_redis_test || test_failed=true
                    fi
                fi
                ;;
            HTTPRoute)
                if [ -z "$G_ITEM" ]; then
                    # Run all HTTPRoute tests
                    run_test "HTTPRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Basic" || test_failed=true
                    run_test "HTTPRoute_SseStreaming" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i SseStreaming" || test_failed=true
                    run_test "HTTPRoute_Delegation" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Delegation" || test_failed=true
                    run_test "HTTPRoute_Match" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Match" || test_failed=true
                    run_test "HTTPRoute_Backend" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend" || test_failed=true
                    if ! should_skip_test "HTTPRoute_Backend_HealthCheckTransition"; then
                        run_test "HTTPRoute_Backend_HealthCheckTransition" "${EDGION_TEST_CLIENT_BIN} -g health-check-transition" || test_failed=true
                    else
                        log_info "Skipping slow test: HTTPRoute_Backend_HealthCheckTransition (use --full-test to run)"
                    fi
                    run_test "HTTPRoute_Filters" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Filters" || test_failed=true
                    run_test "HTTPRoute_Protocol_WebSocket" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Protocol/WebSocket" || test_failed=true
                    run_test "HTTPRoute_Protocol_H2C" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Protocol/H2C" || test_failed=true
                    run_test "HTTPRoute_Protocol_GrpcWeb" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Protocol/GrpcWeb" || test_failed=true
                    run_test "HTTPRoute_Protocol_H2TrailerMalformed" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Protocol/H2TrailerMalformed" || test_failed=true
                    run_test "HTTPRoute_Transport" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Transport" || test_failed=true
                    run_test "HTTPRoute_MultiPort" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i MultiPort" || test_failed=true
                else
                    # Run specified sub-item test
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "HTTPRoute_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            GRPCRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "GRPCRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Basic" || test_failed=true
                    run_test "GRPCRoute_Delegation" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Delegation" || test_failed=true
                    run_test "GRPCRoute_Match" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Match" || test_failed=true
                    run_test "GRPCRoute_RateLimitLocal" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i RateLimitLocal" || test_failed=true
                    run_test "GRPCRoute_TimeoutRewriteWire" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i TimeoutRewriteWire" || test_failed=true
                    run_test "GRPCRoute_Backend_SessionPersistence" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Backend/SessionPersistence" || test_failed=true
                    run_test "GRPCRoute_Backend_SessionPersistenceWeighted" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Backend/SessionPersistenceWeighted" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "GRPCRoute_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            TCPRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "TCPRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i Basic" || test_failed=true
                    run_test "TCPRoute_StreamPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i StreamPlugins" || test_failed=true
                    run_test "TCPRoute_GlobalConnectionIpRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i GlobalConnectionIpRestriction" || test_failed=true
                    run_test "TCPRoute_IdleTimeout" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i IdleTimeout" || test_failed=true
                    run_test "TCPRoute_DuplexBackpressure" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i DuplexBackpressure" || test_failed=true
                    run_test "TCPRoute_GeoRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i GeoRestriction" || test_failed=true
                    run_test "TCPRoute_DeniedBackendRef" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i DeniedBackendRef" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "TCPRoute_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            TLSRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "TLSRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i Basic" || test_failed=true
                    run_test "TLSRoute_ProxyProtocol" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i ProxyProtocol" || test_failed=true
                    run_test "TLSRoute_StreamPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i StreamPlugins" || test_failed=true
                    run_test "TLSRoute_MultiSNI" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i MultiSNI" || test_failed=true
                    run_test "TLSRoute_BothAbsentParentRef" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i BothAbsentParentRef" || test_failed=true
                    run_test "TLSRoute_Passthrough" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i Passthrough" || test_failed=true
                    run_test "TLSRoute_GeoRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i GeoRestriction" || test_failed=true
                    run_test "TLSRoute_DeniedBackendRef" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i DeniedBackendRef" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "TLSRoute_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            UDPRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "UDPRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g -r UDPRoute -i Basic" || test_failed=true
                    run_test "UDPRoute_StreamPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r UDPRoute -i StreamPlugins" || test_failed=true
                    run_test "UDPRoute_DeniedBackendRef" "${EDGION_TEST_CLIENT_BIN} -g -r UDPRoute -i DeniedBackendRef" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "UDPRoute_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r UDPRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            EdgionBackendTrafficPolicy)
                if [ -z "$G_ITEM" ]; then
                    # Control-plane group: the suite applies/edits/deletes policies at
                    # runtime and asserts the Gateway store/config state. No item split.
                    run_test "EdgionBackendTrafficPolicy" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackendTrafficPolicy" || test_failed=true
                else
                    # e.g. -i upstream_authority: EdgionBackendTrafficPolicy/UpstreamAuthority
                    # data-plane suite (dynamic upstream authority, Host/SNI dispatch).
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "EdgionBackendTrafficPolicy_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackendTrafficPolicy -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            Gateway)
                if [ -z "$G_ITEM" ]; then
                    run_test "Gateway_Security" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i Security" || test_failed=true
                    run_test "Gateway_RealIP" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i RealIP" || test_failed=true
                    run_test "Gateway_TLS_BackendTLS" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/BackendTLS" || test_failed=true
        run_test "Gateway_TLS_BackendTLSGatewayCert" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/BackendTLSGatewayCert" || test_failed=true
        run_test "Gateway_TLS_MisdirectedRequest" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/MisdirectedRequest" || test_failed=true
                    run_test "Gateway_TLS_GatewayTLS" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/GatewayTLS" || test_failed=true
                    run_test "Gateway_ListenerHostname" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i ListenerHostname" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Same" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i AllowedRoutes/Same" || test_failed=true
                    run_test "Gateway_AllowedRoutes_All" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i AllowedRoutes/All" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Kinds" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i AllowedRoutes/Kinds" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Selector" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i AllowedRoutes/Selector" || test_failed=true
                    run_test "Gateway_Combined" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i Combined" || test_failed=true
                    run_test "Gateway_StreamPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i StreamPlugins" || test_failed=true
                    run_test "Gateway_TLS_BackendTLSSAN" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/BackendTLSSAN" || test_failed=true
                    run_test "Gateway_TLS_NoHostnameListener" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/NoHostnameListener" || test_failed=true
                    run_test "Gateway_FrontendValidation" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i FrontendValidation" || test_failed=true
                    run_test "Gateway_PortConflict" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i PortConflict" || test_failed=true
                elif [ "$G_ITEM" = "RetainedListenerHandles" ]; then
                    run_test "Dyn_RetainedListenerHandles" \
                        "${EDGION_TEST_CLIENT_BIN} -g --dynamic-scenario RetainedListenerHandles" || test_failed=true
                elif [ "$G_ITEM" = "WafDynamic" ]; then
                    run_test "Dyn_WafDynamic" \
                        "${EDGION_TEST_CLIENT_BIN} -g --dynamic-scenario WafDynamic" || test_failed=true
                else
                    # Replace / with _ in item name for log file
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "Gateway_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            EdgionPlugins)
                if [ -z "$G_ITEM" ]; then
                    run_test "EdgionPlugins_DebugAccessLog" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DebugAccessLog" || test_failed=true
                    run_test "EdgionPlugins_Csrf" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Csrf" || test_failed=true
                    run_test "EdgionPlugins_PluginCondition" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PluginCondition" || test_failed=true
                    run_test "EdgionPlugins_AllConditions" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PluginCondition/AllConditions" || test_failed=true
                    run_test "EdgionPlugins_CtxSet" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i CtxSet" || test_failed=true
                    run_test "EdgionPlugins_JwtAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i JwtAuth" || test_failed=true
                    run_test "EdgionPlugins_JweDecrypt" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i JweDecrypt" || test_failed=true
                    run_test "EdgionPlugins_HmacAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i HmacAuth" || test_failed=true
                    run_test "EdgionPlugins_HeaderCertAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i HeaderCertAuth" || test_failed=true
                    run_test "EdgionPlugins_KeyAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i KeyAuth" || test_failed=true
                    run_test "EdgionPlugins_CredentialInjector" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i CredentialInjector" || test_failed=true
                    run_test "EdgionPlugins_BasicAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i BasicAuth" || test_failed=true
                    if ! should_skip_test "EdgionPlugins_LdapAuth"; then
                        run_test "EdgionPlugins_LdapAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i LdapAuth" || test_failed=true
                    else
                        log_info "Skipping complex test: EdgionPlugins_LdapAuth (requires --full-test)"
                    fi
                    if ! should_skip_test "EdgionPlugins_LdapAuthTls"; then
                        run_test "EdgionPlugins_LdapAuthTls" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i LdapAuthTls" || test_failed=true
                    else
                        log_info "Skipping complex test: EdgionPlugins_LdapAuthTls (requires --full-test)"
                    fi
                    run_test "EdgionPlugins_ProxyRewrite" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ProxyRewrite" || test_failed=true
                    run_test "EdgionPlugins_RateLimitLocal" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RateLimitLocal" || test_failed=true
                    run_test "EdgionPlugins_RateLimitRedis" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RateLimitRedis" || test_failed=true
                    run_token_bucket_partial_refill_redis_test || test_failed=true
                    run_test "EdgionPlugins_RealIp" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RealIp" || test_failed=true
                    run_test "EdgionPlugins_RequestMirror" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RequestMirror" || test_failed=true
                    run_test "EdgionPlugins_ResponseRewrite" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ResponseRewrite" || test_failed=true
                    run_test "EdgionPlugins_RequestRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RequestRestriction" || test_failed=true
                    run_test "EdgionPlugins_RequestAccessPolicy" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RequestAccessPolicy" || test_failed=true
                    run_test "EdgionPlugins_Canary" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Canary" || test_failed=true
                    run_test "EdgionPlugins_IpRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i IpRestriction" || test_failed=true
                    run_test "EdgionPlugins_IpRestrictionPartialGood" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i IpRestrictionPartialGood" || test_failed=true
                    run_test "EdgionPlugins_PathNormalization" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PathNormalization" || test_failed=true
                    run_test "EdgionPlugins_PathNormalization_ForwardView" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PathNormalization/ForwardView" || test_failed=true
                    run_test "EdgionPlugins_FaultInjection" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i FaultInjection" || test_failed=true
                    run_test "EdgionPlugins_ForwardAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ForwardAuth" || test_failed=true
                    if $FULL_TEST; then
                        run_test "EdgionPlugins_ExtProc" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ExtProc --full-test" || test_failed=true
                    else
                        run_test "EdgionPlugins_ExtProc" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ExtProc" || test_failed=true
                    fi
                    run_test "EdgionPlugins_AiGuard" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i AiGuard" || test_failed=true
                    run_test "EdgionPlugins_Guardrail" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Guardrail" || test_failed=true
                    run_test "EdgionPlugins_GuardrailEncoding" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i GuardrailEncoding" || test_failed=true
                    run_test "EdgionPlugins_OpenidConnect" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i OpenidConnect" || test_failed=true
                    run_test "EdgionPlugins_BandwidthLimit" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i BandwidthLimit" || test_failed=true
                    run_test "EdgionPlugins_DirectEndpoint" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DirectEndpoint" || test_failed=true
                    run_test "EdgionPlugins_DynamicInternalUpstream" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DynamicInternalUpstream" || test_failed=true
                    run_test "EdgionPlugins_DynamicExternalUpstream" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DynamicExternalUpstream" || test_failed=true
                    run_test "EdgionPlugins_RegionRoute" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RegionRoute" || test_failed=true
                    run_test "EdgionPlugins_GlobalConnectionIpRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i GlobalConnectionIpRestriction" || test_failed=true
                    run_test "EdgionPlugins_GeoRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i GeoRestriction" || test_failed=true
                    run_test "EdgionPlugins_EdgionConfigDataStatus" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i EdgionConfigDataStatus" || test_failed=true
                    run_test "EdgionPlugins_ResolvedRefs" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ResolvedRefs" || test_failed=true
                    run_test "EdgionPlugins_WebhookKeyGet" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i WebhookKeyGet" || test_failed=true
                    run_test "EdgionPlugins_Cors" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Cors" || test_failed=true
                    run_test "EdgionPlugins_Dsl" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Dsl" || test_failed=true
                    run_test "EdgionPlugins_DslTerminate" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DslTerminate" || test_failed=true
                    run_test "EdgionPlugins_DslGrpcError" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DslGrpcError" || test_failed=true
                    run_test "EdgionPlugins_DslBody" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DslBody" || test_failed=true
                    run_test "EdgionPlugins_BodyPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i BodyPlugins" || test_failed=true
                    run_test "EdgionPlugins_JsonSchemaValidation" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i JsonSchemaValidation" || test_failed=true
                    run_test "EdgionPlugins_GrpcJsonTranscoder" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i GrpcJsonTranscoder" || test_failed=true
                    if ! should_skip_test "EdgionPlugins_AllEndpointStatus"; then
                        run_test "EdgionPlugins_AllEndpointStatus" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i AllEndpointStatus" || test_failed=true
                    else
                        log_info "Skipping slow test: EdgionPlugins_AllEndpointStatus (use --full-test to run)"
                    fi
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    local test_name="EdgionPlugins_${item_safe}"
                    if ! should_skip_test "$test_name"; then
                        if $FULL_TEST && [ "$G_ITEM" = "ExtProc" ]; then
                            run_test "$test_name" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ${G_ITEM} --full-test" || test_failed=true
                        else
                            run_test "$test_name" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ${G_ITEM}" || test_failed=true
                        fi
                    else
                        log_info "Skipping complex test: ${test_name} (requires --full-test)"
                    fi
                    if [ "$G_ITEM" = "RateLimitRedis" ]; then
                        run_token_bucket_partial_refill_redis_test || test_failed=true
                    fi
                fi
                ;;
            EdgionTls)
                if [ -z "$G_ITEM" ]; then
                    run_test "EdgionTls_https" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i https" || test_failed=true
                    if $FULL_TEST && [ "${EDGION_TEST_DOWNSTREAM_TLS_OFFLOAD:-0}" = "1" ]; then
                        run_test "DownstreamTlsOffloadTimeout" \
                            "python3 ${SCRIPT_DIR}/test_downstream_tls_offload_timeout.py" || test_failed=true
                    fi
                    run_test "EdgionTls_grpctls" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i grpctls" || test_failed=true
                    run_test "EdgionTls_mTLS" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i mTLS" || test_failed=true
                    run_test "Dyn_AllowedSanRegexIgnoreCase" \
                        "${EDGION_TEST_CLIENT_BIN} -g --dynamic-scenario AllowedSanRegexIgnoreCase" || test_failed=true
                    run_test "EdgionTls_cipher" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i cipher" || test_failed=true
                    run_test "EdgionTls_mtls_failclose" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i mtls_failclose" || test_failed=true
                    run_test "EdgionTls_fallback_sni" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i fallback_sni" || test_failed=true
                    run_test "EdgionTls_BothAbsentParentRef" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i BothAbsentParentRef" || test_failed=true
        run_test "EdgionTls_port_only" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i port_only" || test_failed=true
        run_test "EdgionTls_sni_collision" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i sni_collision" || test_failed=true
        run_test "EdgionTls_wildcard_single_label" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i wildcard_single_label" || test_failed=true
                elif [ "$G_ITEM" = "https" ]; then
                    run_test "EdgionTls_https" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i https" || test_failed=true
                    if $FULL_TEST && [ "${EDGION_TEST_DOWNSTREAM_TLS_OFFLOAD:-0}" = "1" ]; then
                        run_test "DownstreamTlsOffloadTimeout" \
                            "python3 ${SCRIPT_DIR}/test_downstream_tls_offload_timeout.py" || test_failed=true
                    fi
                elif [ "$G_ITEM" = "LastKnownGood" ]; then
                    run_test "Dyn_TlsLastKnownGood" \
                        "${EDGION_TEST_CLIENT_BIN} -g --dynamic-scenario TlsLastKnownGood" || test_failed=true
                elif [ "$G_ITEM" = "mTLS" ]; then
                    run_test "EdgionTls_mTLS" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i mTLS" || test_failed=true
                    run_test "Dyn_AllowedSanRegexIgnoreCase" \
                        "${EDGION_TEST_CLIENT_BIN} -g --dynamic-scenario AllowedSanRegexIgnoreCase" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "EdgionTls_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            Status)
                if [ -z "$G_ITEM" ]; then
                    run_test "Status_RefGrant" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i RefGrant" || test_failed=true
                    run_test "Status_RefGrant_TCPRouteCrossNsSvc" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i RefGrant/TCPRouteCrossNsSvc" || test_failed=true
                    run_test "Status_RefGrant_TLSRouteCrossNsSvc" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i RefGrant/TLSRouteCrossNsSvc" || test_failed=true
                    run_test "Status_BackendTLSPolicy" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i BackendTLSPolicy" || test_failed=true
                    run_test "Status_EdgionBackendTrafficPolicy" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i EdgionBackendTrafficPolicy" || test_failed=true
                    run_test "Status_GatewayFrontendValidation" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i GatewayFrontendValidation" || test_failed=true
                    run_test "Status_TLSListenerMode" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i TLSListenerMode" || test_failed=true
                    run_test "Status_TLSRoute" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i TLSRoute" || test_failed=true
        run_test "Status_GRPCRoute" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i GRPCRoute" || test_failed=true
        run_test "Status_HostnameIntersection" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i HostnameIntersection" || test_failed=true
        run_test "Status_RefGrant_EdgionConfigData" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i RefGrant/EdgionConfigData" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "Status_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            "LinkSys")
                if [ -z "$G_ITEM" ]; then
                    run_test "LinkSys_Status" "${EDGION_TEST_CLIENT_BIN} -g -r LinkSys -i Status" || test_failed=true
                    run_test "LinkSys_Redis" "$(linksys_redis_cmd)" || test_failed=true
                elif [ "$G_ITEM" = "Redis" ] || [ "$G_ITEM" = "RedisTls" ]; then
                    run_test "LinkSys_Redis" "$(linksys_redis_cmd)" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "LinkSys_${item_safe}" "${EDGION_TEST_CLIENT_BIN} -g -r LinkSys -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            *)
                log_error "Unknown resource type: $G_RESOURCE"
                return 1
                ;;
        esac
    else
        # Run all tests
        log_section "Run Direct mode tests"
        
        run_test "http_direct" "${EDGION_TEST_CLIENT_BIN} http" || test_failed=true
        run_test "grpc_direct" "${EDGION_TEST_CLIENT_BIN} grpc" || test_failed=true
        run_test "websocket_direct" "${EDGION_TEST_CLIENT_BIN} websocket" || test_failed=true
        run_test "tcp_direct" "${EDGION_TEST_CLIENT_BIN} tcp" || test_failed=true
        run_test "udp_direct" "${EDGION_TEST_CLIENT_BIN} udp" || test_failed=true
        
        if $test_failed; then
            log_error "Direct mode tests failed"
            return 1
        fi
        
        log_section "Run Gateway mode tests"

        run_test "EdgionBackend_SingleProvider" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i SingleProvider" || test_failed=true
        run_test "EdgionBackend_Responses" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i Responses" || test_failed=true
        run_test "EdgionBackend_Messages" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i Messages" || test_failed=true
        run_test "EdgionBackend_AnthropicProvider" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i AnthropicProvider" || test_failed=true
        run_test "EdgionBackend_Resilience" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i Resilience" || test_failed=true
        run_test "EdgionBackend_CredentialExhaustion" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i CredentialExhaustion" || test_failed=true
        run_credential_reservation_redis_test || test_failed=true
        
        # HTTPRoute Tests
        run_test "HTTPRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g http" || test_failed=true
        run_test "HTTPRoute_SseStreaming" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i SseStreaming" || test_failed=true
        run_test "HTTPRoute_Delegation" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Delegation" || test_failed=true
        run_test "HTTPRoute_Match" "${EDGION_TEST_CLIENT_BIN} -g http-match" || test_failed=true
        run_test "HTTPRoute_Backend_LBRoundRobin" "${EDGION_TEST_CLIENT_BIN} -g lb-rr" || test_failed=true
        run_test "HTTPRoute_Backend_LBConsistentHash" "${EDGION_TEST_CLIENT_BIN} -g lb-ch" || test_failed=true
        run_test "HTTPRoute_Backend_LBLeastConn" "${EDGION_TEST_CLIENT_BIN} -g lb-lc" || test_failed=true
        run_test "HTTPRoute_Backend_LBLeastConnRetryOnStatus" "${EDGION_TEST_CLIENT_BIN} -g lb-lc-rs" || test_failed=true
        run_test "HTTPRoute_Backend_WeightedBackend" "${EDGION_TEST_CLIENT_BIN} -g weighted-backend" || test_failed=true
        run_test "HTTPRoute_Backend_HealthCheck" "${EDGION_TEST_CLIENT_BIN} -g health-check" || test_failed=true
        if ! should_skip_test "HTTPRoute_Backend_Timeout"; then
            run_test "HTTPRoute_Backend_Timeout" "${EDGION_TEST_CLIENT_BIN} -g timeout" || test_failed=true
        else
            log_info "Skipping slow test: HTTPRoute_Backend_Timeout (use --full-test to run)"
        fi
        if ! should_skip_test "HTTPRoute_Backend_HealthCheckTransition"; then
            run_test "HTTPRoute_Backend_HealthCheckTransition" "${EDGION_TEST_CLIENT_BIN} -g health-check-transition" || test_failed=true
        else
            log_info "Skipping slow test: HTTPRoute_Backend_HealthCheckTransition (use --full-test to run)"
        fi
        run_test "HTTPRoute_Backend_RetryOnStatus" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/RetryOnStatus" || test_failed=true
        if ! should_skip_test "HTTPRoute_Backend_PerTryTimeout"; then
            run_test "HTTPRoute_Backend_PerTryTimeout" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/PerTryTimeout" || test_failed=true
        else
            log_info "Skipping slow test: HTTPRoute_Backend_PerTryTimeout (use --full-test to run)"
        fi

        run_test "HTTPRoute_Backend_InformationalRetry" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/InformationalRetry" || test_failed=true
        run_test "HTTPRoute_Backend_CircuitBreaker" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/CircuitBreaker" || test_failed=true
        run_test "HTTPRoute_Backend_SessionPersistence" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/SessionPersistence" || test_failed=true
        run_test "HTTPRoute_Backend_H2BackendMtlsPersistence" "${EDGION_TEST_CLIENT_BIN} -g h2-mtls-sp" || test_failed=true
        run_test "HTTPRoute_Backend_BackendTLSSNIHostOverride" "${EDGION_TEST_CLIENT_BIN} -g btls-sni" || test_failed=true
        run_test "HTTPRoute_Backend_PartiallyInvalid" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/PartiallyInvalid" || test_failed=true
        run_test "HTTPRoute_Backend_GrpcHealthCheck" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/GrpcHealthCheck" || test_failed=true
        run_test "HTTPRoute_Backend_DnsResolutionFailure" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/DnsResolutionFailure" || test_failed=true
        run_test "HTTPRoute_Backend_DnsResolution" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/DnsResolution" || test_failed=true
        run_test "HTTPRoute_Filters_Redirect" "${EDGION_TEST_CLIENT_BIN} -g http-redirect" || test_failed=true
        run_test "HTTPRoute_Filters_Security" "${EDGION_TEST_CLIENT_BIN} -g http-security" || test_failed=true
        run_test "HTTPRoute_Filters_HeaderModifier" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Filters/HeaderModifier" || test_failed=true
        run_test "HTTPRoute_Filters_FilterConflict" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Filters/FilterConflict" || test_failed=true
        run_test "HTTPRoute_Protocol_WebSocket" "${EDGION_TEST_CLIENT_BIN} -g websocket" || test_failed=true
        run_test "HTTPRoute_Protocol_H2C" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Protocol/H2C" || test_failed=true
        run_test "HTTPRoute_Protocol_GrpcWeb" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Protocol/GrpcWeb" || test_failed=true
        run_test "HTTPRoute_Protocol_H2TrailerMalformed" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Protocol/H2TrailerMalformed" || test_failed=true
        run_test "HTTPRoute_Transport" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Transport" || test_failed=true
        run_test "HTTPRoute_MultiPort" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i MultiPort" || test_failed=true
        run_test "HTTPRoute_Backend_LBEwma" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/LBEwma" || test_failed=true
        run_test "HTTPRoute_Backend_LBEwmaRetryOnStatus" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/LBEwmaRetryOnStatus" || test_failed=true
        run_test "HTTPRoute_Backend_DegradeThreshold" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Backend/DegradeThreshold" || test_failed=true
        run_test "HTTPRoute_DanglingExtensionRef" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i DanglingExtensionRef" || test_failed=true
        run_test "HTTPRoute_Filters_PercentMirror" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Filters/PercentMirror" || test_failed=true

        # EdgionBackendTrafficPolicy Tests
        # Ordered after the HTTPRoute/Backend block on purpose: the control-plane
        # suite has no port_key() and resolves to HTTPRoute/Backend/CircuitBreaker's
        # listener (31302). It is admin-API-only (12001) and cleans its policies up
        # before and after, but running it after CircuitBreaker keeps the shared
        # listener's owner first. The three data-plane suites each override
        # port_key() with a listener of their own (31936 / 31942 / 31943).
        run_test "EdgionBackendTrafficPolicy" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackendTrafficPolicy" || test_failed=true
        run_test "EdgionBackendTrafficPolicy_UpstreamAuthority" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackendTrafficPolicy -i UpstreamAuthority" || test_failed=true
        run_test "EdgionBackendTrafficPolicy_RetryBudget" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackendTrafficPolicy -i RetryBudget" || test_failed=true
        run_test "EdgionBackendTrafficPolicy_ConnectTimeout" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackendTrafficPolicy -i ConnectTimeout" || test_failed=true

        # GRPCRoute Tests
        run_test "GRPCRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Basic" || test_failed=true
        run_test "GRPCRoute_Match" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Match" || test_failed=true
        run_test "GRPCRoute_RateLimitLocal" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i RateLimitLocal" || test_failed=true
        run_test "GRPCRoute_TimeoutRewriteWire" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i TimeoutRewriteWire" || test_failed=true
        run_test "GRPCRoute_Backend_SessionPersistence" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Backend/SessionPersistence" || test_failed=true
        run_test "GRPCRoute_Backend_SessionPersistenceWeighted" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Backend/SessionPersistenceWeighted" || test_failed=true
        run_test "GRPCRoute_Delegation" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i Delegation" || test_failed=true
        run_test "GRPCRoute_UnmatchedRoute" "${EDGION_TEST_CLIENT_BIN} -g -r GRPCRoute -i UnmatchedRoute" || test_failed=true

        # TCPRoute Tests
        run_test "TCPRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i Basic" || test_failed=true
        run_test "TCPRoute_KeepaliveSmoke" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i KeepaliveSmoke" || test_failed=true
        run_test "TCPRoute_StreamPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i StreamPlugins" || test_failed=true
        run_test "TCPRoute_IdleTimeout" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i IdleTimeout" || test_failed=true
        run_test "TCPRoute_DuplexBackpressure" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i DuplexBackpressure" || test_failed=true
        run_test "TCPRoute_GlobalConnectionIpRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i GlobalConnectionIpRestriction" || test_failed=true
        run_test "TCPRoute_GeoRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i GeoRestriction" || test_failed=true
        run_test "TCPRoute_DeniedBackendRef" "${EDGION_TEST_CLIENT_BIN} -g -r TCPRoute -i DeniedBackendRef" || test_failed=true

        # TLSRoute Tests
        run_test "TLSRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i Basic" || test_failed=true
        run_test "TLSRoute_ProxyProtocol" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i ProxyProtocol" || test_failed=true
        run_test "TLSRoute_StreamPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i StreamPlugins" || test_failed=true
        run_test "TLSRoute_MultiSNI" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i MultiSNI" || test_failed=true
        run_test "TLSRoute_BothAbsentParentRef" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i BothAbsentParentRef" || test_failed=true
        run_test "TLSRoute_Passthrough" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i Passthrough" || test_failed=true
        run_test "TLSRoute_UnknownKindBackend" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i UnknownKindBackend" || test_failed=true
        run_test "TLSRoute_DeniedBackendRef" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i DeniedBackendRef" || test_failed=true
        run_test "TLSRoute_GeoRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r TLSRoute -i GeoRestriction" || test_failed=true

        # UDPRoute Tests
        run_test "UDPRoute_Basic" "${EDGION_TEST_CLIENT_BIN} -g -r UDPRoute -i Basic" || test_failed=true
        run_test "UDPRoute_StreamPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r UDPRoute -i StreamPlugins" || test_failed=true
        run_test "UDPRoute_DeniedBackendRef" "${EDGION_TEST_CLIENT_BIN} -g -r UDPRoute -i DeniedBackendRef" || test_failed=true
        
        # Gateway Tests
        run_test "Gateway_Security" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i Security" || test_failed=true
        run_test "Gateway_RealIP" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i RealIP" || test_failed=true
        run_test "Gateway_TLS_BackendTLS" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/BackendTLS" || test_failed=true
        run_test "Gateway_TLS_BackendTLSGatewayCert" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/BackendTLSGatewayCert" || test_failed=true
        run_test "Gateway_TLS_MisdirectedRequest" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/MisdirectedRequest" || test_failed=true
        # EdgionPlugins Tests
        run_test "EdgionPlugins_DebugAccessLog" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DebugAccessLog" || test_failed=true
        run_test "EdgionPlugins_Csrf" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Csrf" || test_failed=true
        run_test "EdgionPlugins_PluginCondition" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PluginCondition" || test_failed=true
        run_test "EdgionPlugins_AllConditions" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PluginCondition/AllConditions" || test_failed=true
        run_test "EdgionPlugins_CtxSet" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i CtxSet" || test_failed=true
        run_test "EdgionPlugins_JwtAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i JwtAuth" || test_failed=true
        run_test "EdgionPlugins_JweDecrypt" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i JweDecrypt" || test_failed=true
        run_test "EdgionPlugins_Wasm" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Wasm" || test_failed=true
        run_test "EdgionPlugins_RequestId" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RequestId" || test_failed=true
        run_test "EdgionPlugins_AccessLogExtern" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i AccessLogExtern" || test_failed=true
        run_test "EdgionPlugins_MatchedRouteMeta" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i MatchedRouteMeta" || test_failed=true
        run_test "EdgionPlugins_IpRestrictionRefs" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i IpRestrictionRefs" || test_failed=true
        run_test "EdgionPlugins_PluginValidationStatus" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PluginValidationStatus" || test_failed=true
        run_test "EdgionPlugins_GlobalPluginsSnapshot" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i GlobalPluginsSnapshot" || test_failed=true
        run_test "EdgionPlugins_EdgionConfigDataManaged" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i EdgionConfigDataManaged" || test_failed=true
        run_test "EdgionPlugins_PathNormalization_PreserveParam" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PathNormalization/PreserveParam" || test_failed=true
        run_test "EdgionPlugins_HmacAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i HmacAuth" || test_failed=true
        run_test "EdgionPlugins_HeaderCertAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i HeaderCertAuth" || test_failed=true
        run_test "EdgionPlugins_KeyAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i KeyAuth" || test_failed=true
        run_test "EdgionPlugins_CredentialInjector" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i CredentialInjector" || test_failed=true
        run_test "EdgionPlugins_BasicAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i BasicAuth" || test_failed=true
        if ! should_skip_test "EdgionPlugins_LdapAuth"; then
            run_test "EdgionPlugins_LdapAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i LdapAuth" || test_failed=true
        else
            log_info "Skipping complex test: EdgionPlugins_LdapAuth (requires --full-test)"
        fi
        if ! should_skip_test "EdgionPlugins_LdapAuthTls"; then
            run_test "EdgionPlugins_LdapAuthTls" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i LdapAuthTls" || test_failed=true
        else
            log_info "Skipping complex test: EdgionPlugins_LdapAuthTls (requires --full-test)"
        fi
        run_test "EdgionPlugins_ProxyRewrite" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ProxyRewrite" || test_failed=true
        run_test "EdgionPlugins_RateLimitLocal" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RateLimitLocal" || test_failed=true
        run_test "EdgionPlugins_RateLimitRedis" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RateLimitRedis" || test_failed=true
        run_token_bucket_partial_refill_redis_test || test_failed=true
        run_test "EdgionPlugins_RealIp" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RealIp" || test_failed=true
        run_test "EdgionPlugins_RequestMirror" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RequestMirror" || test_failed=true
        run_test "EdgionPlugins_ResponseRewrite" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ResponseRewrite" || test_failed=true
        run_test "EdgionPlugins_RequestRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RequestRestriction" || test_failed=true
        run_test "EdgionPlugins_GeoRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i GeoRestriction" || test_failed=true
        run_test "EdgionPlugins_RequestAccessPolicy" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RequestAccessPolicy" || test_failed=true
        run_test "EdgionPlugins_Canary" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Canary" || test_failed=true
        run_test "EdgionPlugins_IpRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i IpRestriction" || test_failed=true
        run_test "EdgionPlugins_IpRestrictionPartialGood" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i IpRestrictionPartialGood" || test_failed=true
        run_test "EdgionPlugins_PathNormalization" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PathNormalization" || test_failed=true
        run_test "EdgionPlugins_PathNormalization_ForwardView" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i PathNormalization/ForwardView" || test_failed=true
        run_test "EdgionPlugins_FaultInjection" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i FaultInjection" || test_failed=true
        run_test "EdgionPlugins_ForwardAuth" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ForwardAuth" || test_failed=true
        if $FULL_TEST; then
            run_test "EdgionPlugins_ExtProc" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ExtProc --full-test" || test_failed=true
        else
            run_test "EdgionPlugins_ExtProc" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ExtProc" || test_failed=true
        fi
        run_test "EdgionPlugins_AiGuard" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i AiGuard" || test_failed=true
        run_test "EdgionPlugins_Guardrail" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Guardrail" || test_failed=true
        run_test "EdgionPlugins_GuardrailEncoding" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i GuardrailEncoding" || test_failed=true
        run_test "EdgionPlugins_OpenidConnect" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i OpenidConnect" || test_failed=true
        run_test "EdgionPlugins_BandwidthLimit" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i BandwidthLimit" || test_failed=true
        run_test "EdgionPlugins_DirectEndpoint" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DirectEndpoint" || test_failed=true
        run_test "EdgionPlugins_DynamicInternalUpstream" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DynamicInternalUpstream" || test_failed=true
        run_test "EdgionPlugins_DynamicExternalUpstream" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DynamicExternalUpstream" || test_failed=true
        run_test "EdgionPlugins_RegionRoute" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i RegionRoute" || test_failed=true
        run_test "EdgionPlugins_GlobalConnectionIpRestriction" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i GlobalConnectionIpRestriction" || test_failed=true
        run_test "EdgionPlugins_EdgionConfigDataStatus" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i EdgionConfigDataStatus" || test_failed=true
        run_test "EdgionPlugins_ResolvedRefs" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i ResolvedRefs" || test_failed=true
        run_test "EdgionPlugins_WebhookKeyGet" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i WebhookKeyGet" || test_failed=true
        run_test "EdgionPlugins_Cors" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Cors" || test_failed=true
        run_test "EdgionPlugins_Dsl" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i Dsl" || test_failed=true
        run_test "EdgionPlugins_DslTerminate" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DslTerminate" || test_failed=true
        run_test "EdgionPlugins_DslGrpcError" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DslGrpcError" || test_failed=true
        run_test "EdgionPlugins_DslBody" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i DslBody" || test_failed=true
        run_test "EdgionPlugins_BodyPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i BodyPlugins" || test_failed=true
        run_test "EdgionPlugins_JsonSchemaValidation" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i JsonSchemaValidation" || test_failed=true
        run_test "EdgionPlugins_GrpcJsonTranscoder" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i GrpcJsonTranscoder" || test_failed=true
        if ! should_skip_test "EdgionPlugins_AllEndpointStatus"; then
            run_test "EdgionPlugins_AllEndpointStatus" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionPlugins -i AllEndpointStatus" || test_failed=true
        else
            log_info "Skipping slow test: EdgionPlugins_AllEndpointStatus (use --full-test to run)"
        fi
        run_test "Gateway_TLS_GatewayTLS" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/GatewayTLS" || test_failed=true
        run_test "Gateway_ListenerHostname" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i ListenerHostname" || test_failed=true
        run_test "Gateway_AllowedRoutes_Same" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i AllowedRoutes/Same" || test_failed=true
        run_test "Gateway_AllowedRoutes_All" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i AllowedRoutes/All" || test_failed=true
        run_test "Gateway_AllowedRoutes_Kinds" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i AllowedRoutes/Kinds" || test_failed=true
        run_test "Gateway_AllowedRoutes_Selector" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i AllowedRoutes/Selector" || test_failed=true
        run_test "Gateway_Combined" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i Combined" || test_failed=true
        run_test "Gateway_StreamPlugins" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i StreamPlugins" || test_failed=true
        run_test "Gateway_TLS_BackendTLSSAN" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/BackendTLSSAN" || test_failed=true
        run_test "Gateway_TLS_NoHostnameListener" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i TLS/NoHostnameListener" || test_failed=true
        run_test "Gateway_FrontendValidation" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i FrontendValidation" || test_failed=true
        run_test "Gateway_PortConflict" "${EDGION_TEST_CLIENT_BIN} -g -r Gateway -i PortConflict" || test_failed=true

        # EdgionTls Tests
        run_test "EdgionTls_https" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i https" || test_failed=true
        run_test "EdgionTls_grpctls" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i grpctls" || test_failed=true
        run_test "EdgionTls_mTLS" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i mTLS" || test_failed=true
        run_test "EdgionTls_cipher" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i cipher" || test_failed=true
        run_test "EdgionTls_mtls_failclose" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i mtls_failclose" || test_failed=true
        run_test "EdgionTls_fallback_sni" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i fallback_sni" || test_failed=true
        run_test "EdgionTls_BothAbsentParentRef" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i BothAbsentParentRef" || test_failed=true
        run_test "EdgionTls_port_only" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i port_only" || test_failed=true
        run_test "EdgionTls_sni_collision" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i sni_collision" || test_failed=true
        run_test "EdgionTls_wildcard_single_label" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionTls -i wildcard_single_label" || test_failed=true
        
        # Controller Status Tests
        run_test "Status_RefGrant" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i RefGrant" || test_failed=true
        run_test "Status_RefGrant_TCPRouteCrossNsSvc" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i RefGrant/TCPRouteCrossNsSvc" || test_failed=true
        run_test "Status_RefGrant_TLSRouteCrossNsSvc" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i RefGrant/TLSRouteCrossNsSvc" || test_failed=true
        run_test "Status_BackendTLSPolicy" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i BackendTLSPolicy" || test_failed=true
        run_test "Status_EdgionBackendTrafficPolicy" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i EdgionBackendTrafficPolicy" || test_failed=true
        run_test "Status_GatewayFrontendValidation" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i GatewayFrontendValidation" || test_failed=true
        run_test "Status_TLSListenerMode" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i TLSListenerMode" || test_failed=true
        run_test "Status_TLSRoute" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i TLSRoute" || test_failed=true
        run_test "Status_GRPCRoute" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i GRPCRoute" || test_failed=true
        run_test "Status_HostnameIntersection" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i HostnameIntersection" || test_failed=true
        run_test "Status_RefGrant_EdgionConfigData" "${EDGION_TEST_CLIENT_BIN} -g -r Status -i RefGrant/EdgionConfigData" || test_failed=true
        run_test "LinkSys_Status" "${EDGION_TEST_CLIENT_BIN} -g -r LinkSys -i Status" || test_failed=true
        run_test "LinkSys_Redis" "$(linksys_redis_cmd)" || test_failed=true

        # Controller Resource-Abnormal Metric (standalone shell test; slow, runs its own controller with tick_secs=30)
        if $FULL_TEST; then
            run_test "ResourceAbnormalMetrics" "bash ${SCRIPT_DIR}/test_resource_abnormal_metrics.sh" || test_failed=true
        else
            log_info "Skipping slow test: ResourceAbnormalMetrics (use --full-test to run)"
        fi

        # Dynamic scenarios (always run as part of full test cycle)
        run_dynamic_scenarios || test_failed=true
    fi

    run_messages_sdk() {
        "$EDGION_MESSAGES_SDK_PYTHON" "$SCRIPT_DIR/test_messages_sdk.py" \
            --ports "$PROJECT_ROOT/edgion-tests/integration/conf/ports.json"
    }

    # Messages checks share the owned topology and finish before residue validation.
    if [ "$G_RESOURCE" = "EdgionBackend" ] && [ "$G_ITEM" = "Messages" ]; then
        run_test "Dyn_AiMessagesBinding" "${EDGION_TEST_CLIENT_BIN} -g --dynamic-scenario AiMessagesBinding" || test_failed=true
        for control in Responses SingleProvider AnthropicProvider; do
            run_test "Messages_Control_${control}" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i ${control}" || test_failed=true
        done
        run_test "Messages_Control_Http" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i Basic" || test_failed=true
        run_test "Messages_Control_Sse" "${EDGION_TEST_CLIENT_BIN} -g -r HTTPRoute -i SseStreaming" || test_failed=true
    fi
    if { [ "$G_RESOURCE" = "EdgionBackend" ] && { [ -z "$G_ITEM" ] || [ "$G_ITEM" = "Messages" ]; }; } || [ -z "$G_RESOURCE" ]; then
        if [ -n "${EDGION_MESSAGES_SDK_PYTHON:-}" ]; then
            run_test "Messages_SDK" "run_messages_sdk" || test_failed=true
        else
            report_test "Messages_SDK" "SKIP" "0" "set EDGION_MESSAGES_SDK_PYTHON to an interpreter with the official anthropic SDK"
        fi
    fi

    run_openai_sdk() {
        "$EDGION_OPENAI_SDK_PYTHON" "$SCRIPT_DIR/test_openai_sdk.py" \
            --ports "$PROJECT_ROOT/edgion-tests/integration/conf/ports.json"
    }
    if $AI_CONFORMANCE; then
        for control in Resilience CredentialExhaustion; do
            run_test "AI_Conformance_${control}" "${EDGION_TEST_CLIENT_BIN} -g -r EdgionBackend -i ${control}" || test_failed=true
        done
        for scenario in AiBackendLifecycle AiBackendSecretRotation AiBackendWeighted AiBackendPolicyIntegration AiGuardOrderingDynamic AiResponsesContinuation; do
            run_test "Dyn_${scenario}" "${EDGION_TEST_CLIENT_BIN} -g --dynamic-scenario ${scenario}" || test_failed=true
        done
        run_test "OpenAI_SDK" "run_openai_sdk" || test_failed=true
    fi

    # A resource-wide Gateway sweep loads the dynamic fixtures. An exact
    # Gateway item only loads that item's fixtures and must remain isolated.
    if [ "$G_RESOURCE" = "Gateway" ] && [ -z "$G_ITEM" ]; then
        run_dynamic_scenarios || test_failed=true
    fi

    if $FULL_TEST && [ "$G_RESOURCE" = "EdgionPlugins" ] \
        && { [ -z "$G_ITEM" ] || [ "$G_ITEM" = "ExtProc" ]; }; then
        run_test "Dyn_ExtProcTlsPolicyRecovery" \
            "${EDGION_TEST_CLIENT_BIN} -g --dynamic-scenario ExtProcTlsPolicyRecovery" || test_failed=true
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

    # Coverage mode notification (heavy lifting done by prepare.sh / start_all.sh)
    if [ "${EDGION_COVERAGE:-0}" = "1" ]; then
        log_info "Coverage mode: profraw will accumulate under target/"
        log_info "Run './cicd/coverage.sh report' after this completes to generate HTML"
    fi

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
            --ai-conformance)
                AI_CONFORMANCE=true
                shift
                ;;
            --full-test)
                FULL_TEST=true
                shift
                ;;
            --jobs)
                INTEGRATION_JOBS="$2"
                shift 2
                ;;
            --isolated-timeout-secs)
                if [ $# -lt 2 ]; then log_error "Missing argument for --isolated-timeout-secs"; exit 2; fi
                ISOLATED_SCENARIO_TIMEOUT_SECONDS="$2"
                shift 2
                ;;
            --isolated-phase-timeout-secs)
                if [ $# -lt 2 ]; then log_error "Missing argument for --isolated-phase-timeout-secs"; exit 2; fi
                ISOLATED_PHASE_TIMEOUT_SECONDS="$2"
                shift 2
                ;;
            --controller-grpc-port)
                CONTROLLER_GRPC_PORT="$2"
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
            --skip-residue-check)
                SKIP_RESIDUE_CHECK=true
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

    if $AI_CONFORMANCE; then
        if [ "$G_RESOURCE" != "EdgionBackend" ] || [ "$G_ITEM" != "Messages" ] || [ -n "$suites" ] || ! $do_start || ! $DO_CLEANUP || $SKIP_RESIDUE_CHECK || [ "${RESIDUE_MODE:-fail}" != "fail" ]; then
            log_error "--ai-conformance requires the owned Messages topology, default cleanup and strict residue validation"
            exit 2
        fi
        if [ -z "${EDGION_OPENAI_SDK_PYTHON:-}" ] || [ -z "${EDGION_MESSAGES_SDK_PYTHON:-}" ]; then
            log_error "--ai-conformance requires EDGION_OPENAI_SDK_PYTHON and EDGION_MESSAGES_SDK_PYTHON"
            exit 2
        fi
    fi

    case "$INTEGRATION_JOBS" in
        ''|*[!0-9]*)
            log_error "--jobs must be a positive integer"
            exit 2
            ;;
    esac
    if [ "$INTEGRATION_JOBS" -lt 1 ]; then
        log_error "--jobs must be at least 1"
        exit 2
    fi
    local deadline_value deadline_label
    for deadline_label in isolated-scenario isolated-phase deadline-grace; do
        case "$deadline_label" in
            isolated-scenario) deadline_value="$ISOLATED_SCENARIO_TIMEOUT_SECONDS" ;;
            isolated-phase) deadline_value="$ISOLATED_PHASE_TIMEOUT_SECONDS" ;;
            deadline-grace) deadline_value="$DEADLINE_GRACE_SECONDS" ;;
        esac
        case "$deadline_value" in
            ''|*[!0-9]*)
                log_error "${deadline_label} timeout must be a positive integer"
                exit 2
                ;;
        esac
        if [ "$deadline_value" -lt 1 ]; then
            log_error "${deadline_label} timeout must be at least 1"
            exit 2
        fi
    done
    if [ ! -f "$DEADLINE_RUNNER" ]; then
        log_error "Deadline runner not found: ${DEADLINE_RUNNER}"
        exit 2
    fi
    
    # Create durable run evidence before any preflight or isolated scenario.
    REPORT_PHASE="preflight initialization"
    init_pending_report

    # Setup cleanup on exit
    trap cleanup EXIT

    # Fail before building or starting dependencies if a fixed isolated port
    # cannot be bound. Docker-only scenarios are checked after Docker probing.
    if [ -z "$G_RESOURCE" ]; then
        REPORT_PHASE="isolated port preflight"
        log_section "Pre-flight Check: Isolated Ports"
        if ! python3 "${UTILS_DIR}/check_isolated_ports.py" \
            --scenario controller_auth \
            --scenario gateway_admin_auth \
            --scenario fips_policy \
            --scenario geoip_db_states \
            --scenario unknown_config_fields \
            --scenario proxy_protocol_policy_lifecycle; then
            report_test "Preflight_IsolatedPorts" "FAIL" "0"
            exit 1
        fi
        report_test "Preflight_IsolatedPorts" "PASS" "0"
    fi
    
    # Auto-infer suites from resource/item if not explicitly specified
    if [ -z "$suites" ] && [ -n "$G_RESOURCE" ]; then
        # Always include base and HTTPRoute/Basic for dependencies (Service, EndpointSlice, etc.)
        local base_suites="base,HTTPRoute/Basic"
        # EdgionPlugins needs its own base Gateway
        if [ "$G_RESOURCE" = "EdgionPlugins" ]; then
            base_suites="${base_suites},EdgionPlugins/base"
            # When running all EdgionPlugins tests, load all plugin configs
            if [ -z "$G_ITEM" ]; then
                # DslGrpcError's plugin/route fixtures live under
                # EdgionPlugins/Dsl, while their shared gRPC Gateway, Service,
                # and EndpointSlice live under GRPCRoute/Basic. Both halves
                # must be present or the suite either cannot connect or falls
                # through to an unrelated GRPCRoute.
                base_suites="${base_suites},GRPCRoute/Basic,LinkSys/Redis"
                suites="${base_suites},EdgionBackend/SingleProvider,EdgionPlugins/DebugAccessLog,EdgionPlugins/Csrf,EdgionPlugins/PluginCondition,EdgionPlugins/CtxSet,EdgionPlugins/BasicAuth,EdgionPlugins/JwtAuth,EdgionPlugins/JweDecrypt,EdgionPlugins/HmacAuth,EdgionPlugins/HeaderCertAuth,EdgionPlugins/KeyAuth,EdgionPlugins/CredentialInjector,EdgionPlugins/ProxyRewrite,EdgionPlugins/RateLimitLocal,EdgionPlugins/RateLimitRedis,EdgionPlugins/RealIp,EdgionPlugins/RequestMirror,EdgionPlugins/ResponseRewrite,EdgionPlugins/RequestRestriction,EdgionPlugins/RequestAccessPolicy,EdgionPlugins/Canary,EdgionPlugins/IpRestriction,EdgionPlugins/IpRestrictionPartialGood,EdgionPlugins/PathNormalization,EdgionPlugins/PathNormalization/ForwardView,EdgionPlugins/FaultInjection,EdgionPlugins/ForwardAuth,EdgionPlugins/ExtProc,EdgionPlugins/AiGuard,EdgionPlugins/Guardrail,EdgionPlugins/GuardrailEncoding,EdgionPlugins/OpenidConnect,EdgionPlugins/BandwidthLimit,EdgionPlugins/DirectEndpoint,EdgionPlugins/DynamicInternalUpstream,EdgionPlugins/DynamicExternalUpstream,EdgionPlugins/RegionRoute,EdgionPlugins/GlobalConnectionIpRestriction,EdgionPlugins/EdgionConfigDataStatus,EdgionPlugins/ResolvedRefs,EdgionPlugins/WebhookKeyGet,EdgionPlugins/Cors,EdgionPlugins/Dsl,EdgionPlugins/DslBody,EdgionPlugins/BodyPlugins,EdgionPlugins/JsonSchemaValidation,EdgionPlugins/GrpcJsonTranscoder,EdgionPlugins/AllEndpointStatus,EdgionPlugins/Wasm,EdgionPlugins/RequestId,EdgionPlugins/AccessLogExtern,EdgionPlugins/MatchedRouteMeta,EdgionPlugins/IpRestrictionRefs,EdgionPlugins/PluginValidationStatus,EdgionPlugins/GlobalPluginsSnapshot,EdgionPlugins/EdgionConfigDataManaged,EdgionPlugins/PathNormalization/PreserveParam,EdgionPlugins/GeoRestriction"
                if $FULL_TEST; then
                    suites="${suites},EdgionPlugins/LdapAuth,EdgionPlugins/LdapAuthTls"
                fi
            else
                suites="${base_suites}"
                if [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/LdapAuth" ] && ! $FULL_TEST; then
                    log_info "LdapAuth suite requires --full-test, config load will be skipped"
                elif [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/LdapAuthTls" ] && ! $FULL_TEST; then
                    log_info "LdapAuthTls suite requires --full-test, config load will be skipped"
                elif [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/DslTerminate" ]; then
                    suites="${base_suites},EdgionPlugins/Dsl"
                elif [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/DslGrpcError" ]; then
                    suites="${base_suites},GRPCRoute/Basic,LinkSys/Redis,EdgionPlugins/Dsl"
                elif [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/Dsl" ]; then
                    suites="${base_suites},LinkSys/Redis,EdgionPlugins/Dsl"
                elif [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/RateLimitRedis" ]; then
                    # The scenario exercises real Redis counters, so a targeted
                    # run needs both the Redis process and its LinkSys + Secret.
                    suites="${base_suites},LinkSys/Redis,EdgionPlugins/RateLimitRedis"
                elif [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/AiGuard" ]; then
                    suites="${base_suites},EdgionBackend/SingleProvider,EdgionPlugins/AiGuard"
                elif [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/Guardrail" ]; then
                    suites="${base_suites},EdgionBackend/SingleProvider,EdgionPlugins/Guardrail"
                elif [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/GuardrailEncoding" ]; then
                    suites="${base_suites},EdgionBackend/SingleProvider,EdgionPlugins/GuardrailEncoding"
                else
                    suites="${suites},${G_RESOURCE}/${G_ITEM}"
                fi
            fi
        elif [ "$G_RESOURCE" = "Status" ]; then
            if [ -n "$G_ITEM" ]; then
                suites="${base_suites},Status/${G_ITEM}"
            else
                suites="${base_suites},Status/RefGrant,Status/BackendTLSPolicy,Status/EdgionBackendTrafficPolicy,Status/GatewayFrontendValidation,Status/TLSListenerMode,Status/TLSRoute"
            fi
        elif [ "$G_RESOURCE" = "EdgionBackendTrafficPolicy" ]; then
            if [ -n "$G_ITEM" ]; then
                # Data-plane item scenarios (UpstreamAuthority, RetryBudget,
                # ConnectTimeout) ship their own Gateway/Service/HTTPRoute/EBTP
                # fixtures, so the item's conf subtree must be preloaded.
                suites="${base_suites},${G_RESOURCE}/${G_ITEM}"
            else
                # Control-plane suite applies its own policies at runtime; only the
                # base namespaces/services need to be preloaded.
                suites="${base_suites}"
            fi
        elif [ "$G_RESOURCE" = "LinkSys" ]; then
            # LinkSys/Redis assertions hit /api/v1/testing/link-sys/redis/* on the
            # Gateway admin port (covered by base) AND drive real proxy traffic
            # through edgion-plugins-gateway against the rate-limit-redis-* plugins
            # — load EdgionPlugins/base + RateLimitRedis so test 9 has its plugins.
            local linksys_extras="EdgionPlugins/base,EdgionPlugins/RateLimitRedis"
            if [ -z "$G_ITEM" ]; then
                suites="${base_suites},LinkSys/Status,LinkSys/Redis,LinkSys/RedisTls,${linksys_extras}"
            elif [ "$G_ITEM" = "Redis" ] || [ "$G_ITEM" = "RedisTls" ]; then
                suites="${base_suites},LinkSys/Redis,LinkSys/RedisTls,${linksys_extras}"
            else
                suites="${base_suites},LinkSys/${G_ITEM}"
            fi
        elif [ -n "$G_ITEM" ]; then
            if [ "$G_RESOURCE" = "EdgionBackend" ] && [ "$G_ITEM" = "Messages" ]; then
                suites="${base_suites},LinkSys/Redis,EdgionBackend/Messages,EdgionBackend/Responses,EdgionBackend/SingleProvider,EdgionBackend/AnthropicProvider,HTTPRoute/Basic,HTTPRoute/SseStreaming"
                if $AI_CONFORMANCE; then
                    suites="${suites},EdgionBackend/Resilience,EdgionBackend/CredentialExhaustion,EdgionBackend/Lifecycle/DynamicTest,EdgionBackend/Weighted,EdgionBackend/PolicyIntegration,EdgionPlugins/AiGuardOrdering/DynamicTest"
                fi
            elif [ "$G_RESOURCE" = "EdgionBackend" ] && [ "$G_ITEM" = "Responses" ]; then
                suites="${base_suites},LinkSys/Redis,EdgionBackend/Responses"
            elif [ "$G_RESOURCE" = "Gateway" ] && [ "$G_ITEM" = "WafDynamic" ]; then
                suites="${base_suites},EdgionPlugins/base,EdgionPlugins/Waf/DynamicTest"
            elif [ "$G_RESOURCE" = "Gateway" ] && [ "$G_ITEM" = "TLS/BackendTLS" ]; then
                suites="${base_suites},HTTPRoute/Backend/BackendTLS"
            elif [ "$G_RESOURCE" = "Gateway" ] && [ "$G_ITEM" = "TLS/BackendTLSSAN" ]; then
                suites="${base_suites},HTTPRoute/Backend/BackendTLSSAN,HTTPRoute/Backend/BackendTLS"
            elif [ "$G_RESOURCE" = "TCPRoute" ] && [ "$G_ITEM" = "StreamPlugins" ]; then
                # The ConnectionRateLimit Redis scenario (09_*) references the
                # default/redis-test LinkSys CRD, which only the LinkSys/Redis
                # suite defines — load it so targeted runs have the Redis client.
                suites="${base_suites},LinkSys/Redis,${G_RESOURCE}/${G_ITEM}"
            elif [ "$G_RESOURCE" = "TLSRoute" ] && [ "$G_ITEM" = "ProxyProtocol" ]; then
                # The negative case connects to the Basic TLSRoute listener.
                suites="${base_suites},TLSRoute/Basic,TLSRoute/ProxyProtocol"
            else
                suites="${base_suites},${G_RESOURCE}/${G_ITEM}"
            fi
        fi
    fi

    local external_ext_proc_required=false
    local external_ext_proc_bin=""
    if is_external_ext_proc_required "$suites"; then
        external_ext_proc_required=true
        external_ext_proc_bin=$(external_ext_proc_binary_path)
        export EDGION_EXT_PROC_EXTERNAL_REQUIRED=1
        export EDGION_EXT_PROC_EXTERNAL_ADMIN_URL="$EXT_PROC_EXTERNAL_ADMIN_URL"

        # --no-prepare is strictly build-free. Validate its required artifact
        # before Docker, Redis, LDAP, or any Edgion service can be started.
        if ! $do_prepare && $do_start; then
            if ! validate_external_ext_proc_artifact "$external_ext_proc_bin"; then
                report_test "Preflight_ExternalExtProcArtifact" "FAIL" "0"
                exit 1
            fi
        fi

        # --no-start owns no process lifecycle. Reuse only a previously loaded
        # work directory whose live endpoint proves the exact pinned identity.
        if ! $do_start; then
            if ! validate_reused_external_ext_proc; then
                report_test "Preflight_ExternalExtProcReuse" "FAIL" "0"
                exit 1
            fi
        fi
    else
        unset EDGION_EXT_PROC_EXTERNAL_REQUIRED
        unset EDGION_EXT_PROC_EXTERNAL_ADMIN_URL
        unset EDGION_EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR
    fi

    # Fast mode guard: LDAP integration test is complex (Docker-required), full-test only.
    if ! $FULL_TEST && [ "$G_RESOURCE" = "EdgionPlugins" ] && [ "$G_ITEM" = "LdapAuth" ]; then
        log_info "EdgionPlugins/LdapAuth is a complex Docker-based test and requires --full-test"
        report_test "Target_EdgionPlugins_LdapAuth" "SKIP" "0" "requires --full-test"
        finalize_report "PASS"
        DO_CLEANUP=false
        exit 0
    fi

    # Fast mode guard: LDAP TLS integration test requires Docker, full-test only.
    if ! $FULL_TEST && [ "$G_RESOURCE" = "EdgionPlugins" ] && [ "$G_ITEM" = "LdapAuthTls" ]; then
        log_info "EdgionPlugins/LdapAuthTls is a complex Docker-based test and requires --full-test"
        report_test "Target_EdgionPlugins_LdapAuthTls" "SKIP" "0" "requires --full-test"
        finalize_report "PASS"
        DO_CLEANUP=false
        exit 0
    fi

    if is_ldap_service_required "$suites"; then
        LDAP_SERVICE_REQUIRED=true
    fi

    if is_ldap_tls_service_required "$suites"; then
        LDAP_TLS_SERVICE_REQUIRED=true
    fi

    if is_redis_service_required "$suites"; then
        REDIS_SERVICE_REQUIRED=true
    fi
    if is_redis_tls_service_required "$suites"; then
        REDIS_TLS_SERVICE_REQUIRED=true
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

    local gateway_bin_dir="${PROJECT_ROOT}/target/debug"
    if [ "${EDGION_COVERAGE:-0}" = "1" ]; then
        gateway_bin_dir="${PROJECT_ROOT}/target/llvm-cov-target/debug"
    fi
    export EDGION_GATEWAY_BIN="${gateway_bin_dir}/edgion-gateway"
    export EDGION_TEST_CLIENT_BIN="${gateway_bin_dir}/test_client"
    export EDGION_CACHE_RESIDUE_BIN="${gateway_bin_dir}/cache_residue"
    export EDGION_CREDENTIAL_RESERVATION_TEST_BIN="${gateway_bin_dir}/credential_reservation_redis"
    export EDGION_TOKEN_BUCKET_PARTIAL_REFILL_TEST_BIN="${gateway_bin_dir}/token_bucket_partial_refill_redis"
    export EDGION_CAPABILITY_REGISTRY="${PROJECT_ROOT}/edgion-tests/integration/capabilities.tsv"
    if [ -n "$G_RESOURCE" ] && [ -n "$G_ITEM" ]; then
        export EDGION_INTEGRATION_EXPLICIT_TARGET=1
    else
        export EDGION_INTEGRATION_EXPLICIT_TARGET=0
    fi

    # =============================================================================
    # Pre-flight Check 0: Only English letter scripts allowed outside docs/ and tasks/
    # =============================================================================
    REPORT_PHASE="English-only preflight"
    log_section "Pre-flight Check: English-Only Lint"
    if ! "${PROJECT_ROOT}/cicd/checks/check_english_only.sh"; then
        log_error "English-only check failed — translate offending text to English before running integration tests"
        report_test "Preflight_EnglishOnly" "FAIL" "0"
        exit 1
    fi
    log_success "English-only check passed"
    report_test "Preflight_EnglishOnly" "PASS" "0"

    # =============================================================================
    # Pre-flight Check 1: Agent-doc validation (links, frontmatter, indexes, ...)
    # =============================================================================
    REPORT_PHASE="agent-doc preflight"
    log_section "Pre-flight Check: Agent-doc Validation"
    if ! python3 "${PROJECT_ROOT}/cicd/checks/validate_agent_docs.py"; then
        log_error "Agent-doc validation failed — fix doc consistency issues before running integration tests"
        report_test "Preflight_AgentDocs" "FAIL" "0"
        exit 1
    fi
    log_success "Agent-doc validation passed"
    report_test "Preflight_AgentDocs" "PASS" "0"

    # =============================================================================
    # Pre-flight Check 2: edgion-resources leaf purity (no crate::core:: edges)
    # =============================================================================
    REPORT_PHASE="types-purity preflight"
    log_section "Pre-flight Check: Types Purity"
    if ! "${PROJECT_ROOT}/cicd/checks/check_types_purity.sh"; then
        log_error "Types-purity check failed — edgion-resources must stay a clean leaf (no crate::core:: edges)"
        report_test "Preflight_TypesPurity" "FAIL" "0"
        exit 1
    fi
    log_success "Types-purity check passed"
    report_test "Preflight_TypesPurity" "PASS" "0"

    # =============================================================================
    # Pre-flight Check 3: shared shell portability
    # =============================================================================
    REPORT_PHASE="Bash portability preflight"
    log_section "Pre-flight Check: Bash Portability"
    if ! "${PROJECT_ROOT}/cicd/checks/check_bash_set_e_increment.sh"; then
        log_error "Bash portability check failed -- replace bare arithmetic increments with assignments"
        report_test "Preflight_BashPortability" "FAIL" "0" "increment guard"
        exit 1
    fi
    if ! "${PROJECT_ROOT}/cicd/checks/tests/shell_compat_test.sh"; then
        log_error "Shared shell compatibility self-test failed"
        report_test "Preflight_BashPortability" "FAIL" "0" "shell compatibility"
        exit 1
    fi
    log_success "Bash portability check passed"
    report_test "Preflight_BashPortability" "PASS" "0"

    # Complete the canonical build and validate every artifact before changing
    # host networking or starting any external dependency or test process.
    REPORT_PHASE="build"
    if $do_prepare; then
        log_section "Step 1: Build all components"
        local prepare_args=("${UTILS_DIR}/prepare.sh")
        if $external_ext_proc_required; then
            prepare_args+=("--full-test")
        fi
        if ! "${prepare_args[@]}"; then
            log_error "Build failed"
            report_test "Build" "FAIL" "0"
            exit 1
        fi
        log_success "Build completed"
        report_test "Build" "PASS" "0"
    else
        log_info "Skip build step"
        report_test "Build" "SKIP" "0" "--no-prepare"
    fi

    if $external_ext_proc_required && $do_start; then
        if ! validate_external_ext_proc_artifact "$external_ext_proc_bin"; then
            report_test "Preflight_ExternalExtProcArtifact" "FAIL" "0"
            exit 1
        fi
        report_test "Preflight_ExternalExtProcArtifact" "PASS" "0"
    fi

    REPORT_PHASE="binary capability validation"
    if [ ! -x "$EDGION_GATEWAY_BIN" ]; then
        log_error "Selected Gateway binary is missing: ${EDGION_GATEWAY_BIN}"
        report_test "Preflight_BinaryCapabilities" "FAIL" "0" "Gateway binary missing"
        exit 1
    fi
    local gateway_capabilities
    if ! gateway_capabilities=$("$EDGION_GATEWAY_BIN" --print-build-capabilities); then
        log_error "Failed to read build capabilities from ${EDGION_GATEWAY_BIN}"
        report_test "Preflight_BinaryCapabilities" "FAIL" "0" "Gateway capability query"
        exit 1
    fi
    if ! echo "$gateway_capabilities" | grep -q '^format=edgion-build-capabilities/v1$'; then
        log_error "Unsupported Gateway capability format"
        report_test "Preflight_BinaryCapabilities" "FAIL" "0" "unsupported format"
        exit 1
    fi
    if ! echo "$gateway_capabilities" | grep -q '^body-buffering=enabled$'; then
        log_error "Gateway does not report body-buffering=enabled"
        report_test "Preflight_BinaryCapabilities" "FAIL" "0" "body buffering disabled"
        exit 1
    fi
    export EDGION_TEST_GATEWAY_CAPABILITIES="$gateway_capabilities"
    log_success "Gateway capabilities verified"

    if [ ! -x "$EDGION_TEST_CLIENT_BIN" ]; then
        log_error "Integration test client is missing: ${EDGION_TEST_CLIENT_BIN}"
        report_test "Preflight_BinaryCapabilities" "FAIL" "0" "test client missing"
        exit 1
    fi
    if ! "$EDGION_TEST_CLIENT_BIN" --list-dynamic-scenarios > /dev/null; then
        log_error "Integration capability registry validation failed"
        report_test "Preflight_BinaryCapabilities" "FAIL" "0" "scenario enumeration"
        exit 1
    fi
    if [ "$SKIP_RESIDUE_CHECK" != "true" ] && [ ! -x "$EDGION_CACHE_RESIDUE_BIN" ]; then
        log_error "Cache residue validator is missing: ${EDGION_CACHE_RESIDUE_BIN}"
        report_test "Preflight_BinaryCapabilities" "FAIL" "0" "cache residue validator missing"
        exit 1
    fi
    if { [ -z "$G_RESOURCE" ] \
        || { [ "$G_RESOURCE" = "EdgionBackend" ] \
            && { [ -z "$G_ITEM" ] || [ "$G_ITEM" = "CredentialExhaustion" ]; }; }; } \
        && [ ! -x "$EDGION_CREDENTIAL_RESERVATION_TEST_BIN" ]; then
        log_error "Credential reservation Redis test executable is missing: ${EDGION_CREDENTIAL_RESERVATION_TEST_BIN}"
        report_test "Preflight_BinaryCapabilities" "FAIL" "0" "credential reservation test missing"
        exit 1
    fi
    if { [ -z "$G_RESOURCE" ] \
        || { [ "$G_RESOURCE" = "EdgionPlugins" ] \
            && { [ -z "$G_ITEM" ] || [ "$G_ITEM" = "RateLimitRedis" ]; }; }; } \
        && [ ! -x "$EDGION_TOKEN_BUCKET_PARTIAL_REFILL_TEST_BIN" ]; then
        log_error "Token-bucket partial-refill Redis test executable is missing: ${EDGION_TOKEN_BUCKET_PARTIAL_REFILL_TEST_BIN}"
        report_test "Preflight_BinaryCapabilities" "FAIL" "0" "token bucket partial refill test missing"
        exit 1
    fi
    report_test "Preflight_BinaryCapabilities" "PASS" "0"

    REPORT_PHASE="file-descriptor preflight"
    if ! ensure_nofile_limit "$REQUIRED_NOFILE"; then
        log_error "File descriptor limit check failed"
        report_test "Preflight_FileDescriptorLimit" "FAIL" "0"
        exit 1
    fi
    report_test "Preflight_FileDescriptorLimit" "PASS" "0"

    # These explicitly selected suites use only 127.0.0.1. Do not require the
    # macOS-only aliases that belong to unrelated LB and GeoIP suites.
    if { [ "$G_RESOURCE" = "EdgionBackend" ] && { [ -z "$G_ITEM" ] || [ "$G_ITEM" = "SingleProvider" ] || [ "$G_ITEM" = "AnthropicProvider" ] || [ "$G_ITEM" = "Resilience" ] || [ "$G_ITEM" = "CredentialExhaustion" ] || { [ "$G_ITEM" = "Responses" ] || [ "$G_ITEM" = "Messages" ]; }; }; } \
        || { [ "$G_RESOURCE" = "GRPCRoute" ] && [ "$G_ITEM" = "TimeoutRewriteWire" ]; } \
        || { [ "$G_RESOURCE" = "HTTPRoute" ] && { [ "$G_ITEM" = "Backend/InformationalRetry" ] || [ "$G_ITEM" = "Transport" ]; }; } \
        || { [ "$G_RESOURCE" = "Gateway" ] && [ "$G_ITEM" = "WafDynamic" ]; } \
        || { [ "$G_RESOURCE" = "EdgionPlugins" ] && { [ "$G_ITEM" = "Waf" ] || [ "$G_ITEM" = "ForwardAuth" ] || [ "$G_ITEM" = "ExtProc" ] || [ "$G_ITEM" = "BandwidthLimit" ] || [ "$G_ITEM" = "GrpcJsonTranscoder" ]; }; }; then
        log_info "Skipping loopback-alias check for ${G_RESOURCE}${G_ITEM:+/${G_ITEM}}"
        report_test "Preflight_LoopbackAliases" "SKIP" "0" "not required by selected suites"
    elif ! ensure_loopback_aliases; then
        log_error "Loopback alias check failed"
        report_test "Preflight_LoopbackAliases" "FAIL" "0"
        exit 1
    else
        report_test "Preflight_LoopbackAliases" "PASS" "0"
    fi

    REPORT_PHASE="Docker environment preflight"
    if ! check_docker_environment_ready; then
        log_error "Docker environment check failed"
        report_test "Preflight_DockerEnvironment" "FAIL" "0"
        exit 1
    fi
    report_test "Preflight_DockerEnvironment" "PASS" "0"

    # Step 0: Start external dependencies when needed
    REPORT_PHASE="external dependency startup"
    if ! start_ldap_service_if_needed; then
        log_error "Failed to start external LDAP dependency"
        report_test "Preflight_ExternalDependencies" "FAIL" "0" "LDAP startup"
        exit 1
    fi

    if ! start_ldap_tls_service_if_needed; then
        log_error "Failed to start external LDAP TLS dependency"
        report_test "Preflight_ExternalDependencies" "FAIL" "0" "LDAP TLS startup"
        exit 1
    fi

    if ! start_redis_service_if_needed; then
        log_error "Failed to start external Redis dependency"
        report_test "Preflight_ExternalDependencies" "FAIL" "0" "Redis startup"
        exit 1
    fi
    report_test "Preflight_ExternalDependencies" "PASS" "0"

    # conf_sync mTLS certificates are generated per work directory by the
    # script that creates it (start_all_with_conf.sh for the main stack; each
    # standalone script for its own WORK_DIR / mktemp dir). Nothing is written
    # to the repo root.

    # Step 1.6: Controller no-auth + token-auth startup (standalone, no Gateway).
    # Only runs in the default full sweep (no -r / -i targeting). Two phases:
    #   Phase 1 (no-auth): no [local_auth]/[auth] + no EDGION_ADMIN_TOKEN →
    #     /health and /api/v1/auth/status 200 (mode "token"); business route 503
    #     fail-close.
    #   Phase 2 (token auth): EDGION_ADMIN_TOKEN injected → business route 401
    #     without a Bearer header (auth active, not 503 fail-close) and 200 with the
    #     correct Bearer token.
    if [ -z "$G_RESOURCE" ]; then
        REPORT_PHASE="isolated scenarios"
        if ! run_isolated_scenarios; then
            log_error "FAIL: isolated standalone scenarios"
            exit 1
        fi
    fi

    # Step 1.7: Gateway admin API auth (standalone preflight, boots its own
    # minimal Controller as the gRPC sync backend). Only runs in the default full
    # sweep (no -r / -i targeting). Verifies: no token configured ->
    # /configclient/* 503 fail-close (+ startup WARN); token configured -> 401
    # without a header and 200 with the correct Bearer token; the probe port
    # (/health, /ready) stays auth-free (200 without any token).

    # Step 1.8: FIPS-approved TLS policy (standalone, boots its own minimal
    # Controller + Gateway with EDGION_FIPS_POLICY=on on isolated ports). Only runs
    # in the default full sweep (no -r / -i targeting). Isolated on purpose: the
    # FIPS policy toggle is process-global, so it must not run on the main sweep's
    # single shared gateway. Verifies a ChaCha20-only TLS 1.2 client is rejected
    # and an approved AES-GCM client is accepted.

    # Step 1.9: GeoIP database-state matrix (standalone, boots its own minimal
    # Controller + Gateway on isolated 586xx ports, restarted once per phase
    # against the same Controller). Only runs in the default full sweep (no
    # -r / -i targeting). Isolated on purpose: geoip is process-global and
    # restart-only (edgion-gateway/src/geoip/config.rs), so "no database",
    # "Country-tier database", "City-tier database", and "updater hot-swap"
    # are four mutually exclusive configurations of one gateway process and
    # cannot coexist with the shared City-tier gateway the main sweep uses.
    # Verifies: startup never blocks/exits on a missing/absent database;
    # failOpen:false rejects in both Blacklist and Whitelist mode when the
    # database is unavailable; failOpen:true passes; an L4 profile with
    # failOpen:false denies and bumps edgion_geoip_denied_total{reason="db_unavailable"}
    # while failOpen:true admits; Country-tier country rules work and a city
    # rule's predicate is simply absent (no "database tier" concept; onMissing
    # governs); City-tier coordinate emission
    # emits a masked geo.remote.coord in the access log; and the updater
    # hot-swap end to end (download -> validate -> rename -> re-mmap -> ArcSwap)
    # turns a previously-inert city rule live (the behavioral proof the swap
    # actually happened — there is no admin-endpoint snapshot), writes
    # <db_path>.meta, and leaves no <db_path>.tmp.

    # EGC plugin-policy edits run as PluginPolicyDynamic on the shared topology.

    # Step 1.11: Unknown config fields -- presence proof (standalone, boots its
    # own minimal file_system Controller, no gateway, no K8s, on isolated
    # 586xx ports). Only runs in the default full sweep (no -r / -i
    # targeting). The suite guard (verify_no_unknown_config_fields above)
    # proves ABSENCE across the shared sweep; this proves PRESENCE end to
    # end: an EdgionPlugins fixture with `config.denys` (typo for `deny`) is
    # reported with the exact path while the resource stays Accepted: True
    # (warn-not-reject); a ForwardAuth `successStatusCodes` fixture is
    # reported (the `#[serde(flatten)]` case `deny_unknown_fields` cannot
    # catch structurally); an EdgionStreamPlugins typo and a typed
    # EdgionConfigData typo are reported too, while a Misc ConfigData payload
    # remains intentionally open;
    # corrected YAML drains the diagnostics endpoint to zero AND removes the
    # `UnknownConfigFields` status condition (not just upsert); deletion
    # leaves no ghost entry; and `POST /api/v1/validate/{kind}` reports a
    # typo without persisting anything.

    # Inbound PROXY protocol (standalone, boots its own stack on isolated 584xx
    # ports; the chain suite uses 585xx). Isolated on purpose: a listener with
    # `edgion.io/inbound-proxy-protocol` in required mode rejects every connection that does not
    # begin with a header, so it is unusable by any suite sharing the stack, and
    # the flag is read once at bind time so it cannot be toggled between phases.
    #
    # Both drive a real HAProxy over docker rather than hand-assembled bytes and
    # first verify on the wire which version is actually being emitted, so a
    # misconfigured leg cannot silently cover the wrong parser path. The inbound
    # suite drives both v1 and v2; the chain suite drives v2 only, since what it
    # tests is address propagation across hops, not the parsers. Skipped where
    # docker is unavailable rather than failing.

    # The separate proxy_protocol_policy_lifecycle scenario uses the file-system
    # Controller on isolated 589xx ports. It covers ECD visibility/grant
    # authorization, invalid startup snapshots, and restart-scoped inline/ref
    # policy transitions without depending on Docker. It refuses to execute
    # stale binaries, so a source/build mismatch is an infrastructure failure.
    # Admin token for the main suite (token-only model).
    #
    # The Controller authenticates the Admin API with a single pre-shared bearer
    # token read from EDGION_ADMIN_TOKEN. With no token configured it fail-closes
    # (503) on every business route, so config load via edgion-cli would fail.
    # Export an obviously-fake fixed test token here so all sides line up: the
    # Controller (started by start_all_with_conf.sh below) registers it as the
    # allow-all admin token; edgion-cli (load_conf.sh + every later resource test)
    # presents it via EDGION_TOKEN; and the bash admin helpers / test_client present
    # it via EDGION_TEST_ADMIN_TOKEN. This is the single source of truth for the
    # controller-admin token — start_all_with_conf.sh falls back to the same literal
    # only when run standalone. Exported AFTER the no-auth preflight (Step 1.6/1.7)
    # on purpose so those clean-env scenarios are unaffected.
    export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
    export EDGION_TOKEN="${EDGION_ADMIN_TOKEN}"
    export EDGION_TEST_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN}"

    # Step 2: Start services (including config load)
    REPORT_PHASE="shared stack startup"
    if $do_start; then
        log_section "Step 2: Start all services and load config"
        export EDGION_TEST_CONTROLLER_GRPC_PORT="${CONTROLLER_GRPC_PORT}"
        if [ -z "$G_RESOURCE" ] || [ "$G_RESOURCE" = "EdgionBackend" ] || [[ ",${suites}," == *",EdgionBackend/SingleProvider,"* ]] || [[ ",${suites}," == *",EdgionBackend/AnthropicProvider,"* ]] || [[ ",${suites}," == *",EdgionBackend/Resilience,"* ]] || [[ ",${suites}," == *",EdgionBackend/Responses,"* ]] || [[ ",${suites}," == *",EdgionBackend/Messages,"* ]] || [[ ",${suites}," == *",EdgionBackend/CredentialExhaustion,"* ]]; then
            export EDGION_TEST_ENABLE_GENAI_METRICS=1
        else
            unset EDGION_TEST_ENABLE_GENAI_METRICS
        fi

        # Limit GeoIP fixture injection to resource suites that exercise it. With the
        # fixture present, 127.0.0.1 resolves whenever a rule requests a lookup, so a
        # `geo` object may appear in those access-log lines. Existing assertions are
        # substring/key-lookup based and should tolerate the additive field, but this
        # scope keeps a regression cheap to bisect.
        if [ -z "$G_RESOURCE" ] || [ "$G_RESOURCE" = "EdgionPlugins" ] || [ "$G_RESOURCE" = "TCPRoute" ] || [ "$G_RESOURCE" = "TLSRoute" ]; then
            export EDGION_TEST_ENABLE_GEOIP=1
        else
            unset EDGION_TEST_ENABLE_GEOIP
        fi

        # Build start command without shell re-parsing, preserving every path
        # and suite list as one argument.
        local start_args=("${UTILS_DIR}/start_all_with_conf.sh")
        if [ -n "$suites" ]; then
            start_args+=("--suites" "$suites")
        fi
        start_args+=("--controller-grpc-port" "$CONTROLLER_GRPC_PORT")
        if $external_ext_proc_required; then
            start_args+=(
                "--ext-proc-external-bin" "$external_ext_proc_bin"
                "--ext-proc-external-grpc-port" "$EXT_PROC_EXTERNAL_GRPC_PORT"
                "--ext-proc-external-admin-port" "$EXT_PROC_EXTERNAL_ADMIN_PORT"
                "--ext-proc-external-tls-port" "$EXT_PROC_EXTERNAL_TLS_PORT"
                "--ext-proc-external-mtls-port" "$EXT_PROC_EXTERNAL_MTLS_PORT"
                "--ext-proc-external-config-template" \
                    "${PROJECT_ROOT}/edgion-tests/integration/fixtures/ext_proc_external/config.yaml.tmpl"
            )
        fi
        
        # Execute start script
        # NOTE: Must separate 'local' from command substitution, because
        # 'local' always returns 0 and masks the real exit code of $().
        local output exit_code
        if [ -n "$REDIS_NATIVE_CA_FILE" ]; then
            output=$(EDGION_TEST_ENABLE_REDIS_TLS=1 SSL_CERT_FILE="$REDIS_NATIVE_CA_FILE" \
                "${start_args[@]}" 2>&1) \
                && exit_code=0 || exit_code=$?
        else
            output=$("${start_args[@]}" 2>&1) && exit_code=0 || exit_code=$?
        fi
        
        # Show output
        echo "$output"
        
        if [ $exit_code -ne 0 ]; then
            log_error "Start failed (exit code: $exit_code)"
            report_test "Preflight_SharedStack" "FAIL" "0" "startup rc=${exit_code}"
            exit 1
        fi
        
        # Get work directory from .current file (written by start_all_with_conf.sh)
        # This is more robust than 'tail -1' which can be polluted by background
        # process output leaking into the captured stdout.
        local current_file="${PROJECT_ROOT}/tmp/integration_testing/.current"
        if [ -f "$current_file" ]; then
            WORK_DIR=$(cat "$current_file")
        else
            log_error "Work directory file not found: $current_file"
            report_test "Preflight_SharedStack" "FAIL" "0" "work directory missing"
            exit 1
        fi
        
        # Set environment variables
        export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
        export EDGION_WORK_DIR="${WORK_DIR}"
        if $external_ext_proc_required; then
            export EDGION_EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR="${WORK_DIR}/ext-proc-external-tls"
        fi
        
        log_success "All services started, config loaded"
        log_info "Work directory: ${WORK_DIR}"
        report_test "Preflight_SharedStack" "PASS" "0"
    else
        log_info "Skip start step"
        report_test "Preflight_SharedStack" "SKIP" "0" "--no-start"
        if [ -f "${PROJECT_ROOT}/tmp/integration_testing/.current" ]; then
            WORK_DIR=$(cat "${PROJECT_ROOT}/tmp/integration_testing/.current")
            export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
            export EDGION_WORK_DIR="${WORK_DIR}"
            if $external_ext_proc_required; then
                export EDGION_EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR="${WORK_DIR}/ext-proc-external-tls"
            fi
            log_info "Using existing work directory: ${WORK_DIR}"
        fi
    fi

    # Enforce shared-Gateway principle: exactly one edgion-gateway process.
    REPORT_PHASE="shared Gateway process validation"
    if ! check_single_gateway_process; then
        report_test "Preflight_SharedGateway" "FAIL" "0"
        exit 1
    fi
    report_test "Preflight_SharedGateway" "PASS" "0"

    # =============================================================================
    # Fail-fast gate: no unknown config fields before any suite runs
    # =============================================================================
    # If a plugin's config key was dropped by a schema mismatch during config
    # load, every later assertion about that plugin is suspect -- a GREEN run
    # from here is the dangerous outcome, not a safe one. Fail here, in
    # seconds, rather than after a 20-minute suite that passed for the wrong
    # reason. See verify_no_unknown_config_fields above.
    REPORT_PHASE="initial unknown-field validation"
    log_section "Pre-flight Check: No Unknown Config Fields"
    if ! verify_no_unknown_config_fields "http://127.0.0.1:12101"; then
        log_error "Unknown config fields present after initial config load -- aborting before Round 1"
        report_test "Preflight_UnknownConfigFields" "FAIL" "0"
        exit 1
    fi
    report_test "Preflight_UnknownConfigFields" "PASS" "0"

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
            if grep -q '^SKIPPED:' "$log_file"; then
                log_warn "${name} test skipped"
                local skip_detail
                skip_detail=$(grep '^SKIPPED:' "$log_file" | sort -u)
                report_test "$name" "SKIP" "$duration" "$skip_detail"
                return 0
            fi
            local deferred_detail
            deferred_detail=$(grep '^DEFERRED:' "$log_file" | sort -u || true)
            if [ -n "$deferred_detail" ]; then
                log_warn "$deferred_detail"
            fi
            log_success "${name} test passed"
            report_test "$name" "PASS" "$duration" "$deferred_detail"
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
    local controller_url="http://127.0.0.1:12101"

    # =============================================================================
    # Test execution with optional reload
    # =============================================================================
    if $with_reload; then
        # ========== Round 1: Initial tests ==========
        REPORT_PHASE="main tests before reload"
        log_section "📋 Round 1: Initial tests (before reload)"

        local gateway_url="http://127.0.0.1:12001"
        
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
        REPORT_PHASE="Controller and Gateway reload"
        log_section "🔄 Triggering reload"
        
        if ! trigger_reload "$controller_url"; then
            log_error "Failed to trigger reload"
            report_test "Reload_Trigger" "FAIL" "0"
            exit 1
        fi
        
        # Wait for the observable reload state instead of imposing a fixed delay.
        local controller_id_after=""
        local gateway_id_after=""
        reload_server_ids_synced() {
            controller_id_after=$(get_controller_server_id "$controller_url")
            gateway_id_after=$(get_gateway_server_id "$gateway_url")
            [ "$controller_id_after" != "unknown" ] \
                && [ "$gateway_id_after" != "unknown" ] \
                && [ "$controller_id_after" != "$controller_id_before" ] \
                && [ "$gateway_id_after" != "$gateway_id_before" ] \
                && [ "$controller_id_after" = "$gateway_id_after" ]
        }
        log_info "Waiting for Controller reload and Gateway resynchronization..."
        edgion_wait_until 30 1 reload_server_ids_synced || true
        unset -f reload_server_ids_synced

        # ========== Verify Controller server_id changed ==========
        log_info "New Controller server_id: ${controller_id_after}"
        
        if [ "$controller_id_before" = "$controller_id_after" ]; then
            log_error "Controller server_id did not change after reload!"
            log_error "Before: ${controller_id_before}"
            log_error "After:  ${controller_id_after}"
            report_test "Reload_Controller_ServerID_Changed" "FAIL" "0"
            exit 1
        else
            log_success "Controller server_id changed successfully!"
            log_info "Before: ${controller_id_before}"
            log_info "After:  ${controller_id_after}"
            report_test "Reload_Controller_ServerID_Changed" "PASS" "0"
        fi
        
        # ========== Verify Gateway server_id changed ==========
        log_info "New Gateway server_id: ${gateway_id_after}"
        
        if [ "$gateway_id_before" = "$gateway_id_after" ]; then
            log_error "Gateway server_id did not change after reload!"
            log_error "Before: ${gateway_id_before}"
            log_error "After:  ${gateway_id_after}"
            log_error "This means Gateway did not detect the reload and re-sync!"
            report_test "Reload_Gateway_ServerID_Changed" "FAIL" "0"
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
            test_failed=true
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
        REPORT_PHASE="main tests after reload"
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
        REPORT_PHASE="main tests"
        if ! run_all_tests; then
            test_failed=true
        fi
    fi
    
    # =============================================================================
    # Final Step: Metrics Label Whitelist
    # =============================================================================
    REPORT_PHASE="final metrics validation"
    log_section "Final Step: Metrics Label Whitelist"
    if ! verify_metrics_labels "http://127.0.0.1:12190/metrics" "controller"; then
        log_error "Metrics label validation FAILED"
        report_test "Final_MetricsLabelWhitelist" "FAIL" "0"
        test_failed=true
    else
        report_test "Final_MetricsLabelWhitelist" "PASS" "0"
    fi

    # =============================================================================
    # Final Step: No Unknown Config Fields
    # =============================================================================
    # A second pass, distinct from the pre-flight gate above: dynamic/hot-reload
    # scenarios and --with-reload Round 2 apply more resources mid-run, and only
    # a second pass sees those. The registry is cumulative within a run, so this
    # re-reports anything the pre-flight gate already found -- acceptable, and it
    # keeps the two call sites independent.
    REPORT_PHASE="final unknown-field validation"
    log_section "Final Step: No Unknown Config Fields"
    if ! verify_no_unknown_config_fields "http://127.0.0.1:12101"; then
        log_error "Unknown config fields validation FAILED"
        report_test "Final_UnknownConfigFields" "FAIL" "0"
        test_failed=true
    else
        report_test "Final_UnknownConfigFields" "PASS" "0"
    fi

    # =============================================================================
    # Final Step: Requeue Dead Loop Detection
    # =============================================================================
    # Scan controller logs for trigger-chain cycle/depth errors that indicate a
    # requeue dead loop was detected and suppressed. These should never appear in
    # a healthy integration run; any occurrence is a bug in handler requeue logic.
    REPORT_PHASE="final requeue-loop validation"
    log_section "Final Step: Requeue Dead Loop Detection"
    local controller_log="${WORK_DIR}/logs/controller.log"
    if [ -r "$controller_log" ]; then
        local requeue_hits grep_rc
        if requeue_hits=$(grep -c \
            -e "Trigger cycle limit reached, dropping requeue" \
            -e "Trigger chain depth limit reached, dropping requeue" \
            "$controller_log" 2>/dev/null); then
            grep_rc=0
        else
            grep_rc=$?
        fi
        if [ "$grep_rc" -gt 1 ]; then
            log_error "Requeue dead loop check could not read controller log (grep rc=${grep_rc}): ${controller_log}"
            report_test "Final_RequeueDeadLoop" "FAIL" "0" "controller.log read error rc=${grep_rc}"
            test_failed=true
        elif [ "${requeue_hits:-0}" -gt 0 ]; then
            log_error "Requeue dead loop detected: ${requeue_hits} occurrence(s) in controller.log"
            grep \
                -e "Trigger cycle limit reached, dropping requeue" \
                -e "Trigger chain depth limit reached, dropping requeue" \
                "$controller_log" | head -20 || true
            report_test "Final_RequeueDeadLoop" "FAIL" "0" "${requeue_hits} event(s)"
            test_failed=true
        else
            log_success "Requeue dead loop check passed (no dead loop events in controller log)"
            report_test "Final_RequeueDeadLoop" "PASS" "0"
        fi
    else
        log_error "Requeue dead loop check failed — controller.log is missing or unreadable: ${controller_log}"
        report_test "Final_RequeueDeadLoop" "FAIL" "0" "controller.log unavailable"
        test_failed=true
    fi

    # =============================================================================
    # Final Step: Cache Residue Detection (leak detection across all scenarios)
    # =============================================================================
    REPORT_PHASE="final cache-residue validation"
    if [ "$SKIP_RESIDUE_CHECK" != "true" ]; then
        log_section "Final Step: Cache Residue Detection"

        local residue_bin="$EDGION_CACHE_RESIDUE_BIN"
        local residue_infra_error=""
        local residue_mode="${RESIDUE_MODE:-fail}"
        case "$residue_mode" in
            fail|warn)
                ;;
            *)
                residue_infra_error="invalid RESIDUE_MODE=${residue_mode}; expected fail or warn"
                ;;
        esac
        if [ ! -x "$residue_bin" ]; then
            residue_infra_error="precompiled validator binary unavailable"
        fi

        # The validator authenticates to the controller's now-standard-auth
        # diagnostic routes (wipe-all, store-stats), so obtain the admin token.
        # Missing authentication is an infrastructure failure, not a skip.
        if [ -z "$residue_infra_error" ]; then
            if ! ensure_ctrl_admin_token || [ -z "$CTRL_ADMIN_TOKEN" ]; then
                residue_infra_error="controller admin token unavailable"
            fi
        fi

        if [ -n "$residue_infra_error" ]; then
            log_error "Cache residue check failed: ${residue_infra_error}"
            report_test "Final_CacheResidue" "FAIL" "0" "$residue_infra_error"
            test_failed=true
        else
            # Always invoke validator with --mode fail so exit 1 unambiguously means
            # "residue detected". Shell-side RESIDUE_MODE then decides whether to
            # block the build (default: fail — opt out with RESIDUE_MODE=warn).
            # The controller's test-only diagnostic routes (wipe-all, store-stats)
            # require standard auth like any other admin route, so the validator
            # is given the admin Bearer token obtained at login. The gateway admin
            # API also requires a bearer token; the residue validator reads it from
            # the EDGION_GATEWAY_ADMIN_TOKEN env var (exported by start_all_with_conf.sh
            # and inherited here), so gateway-side residue checks run as well.
            # Disable errexit around the call so a non-zero exit (residue / infra
            # error) is captured into $rc and handled below instead of aborting.
            # Allow resource propagation, background cancellation, and in-flight
            # leases to settle before classifying retained handles as a leak.
            set +e
            "$residue_bin" \
                --controller-addr "http://127.0.0.1:12101" \
                --gateway-addr    "http://127.0.0.1:12001" \
                --controller-token "$CTRL_ADMIN_TOKEN" \
                --timeout-secs 75 \
                --mode fail
            local rc=$?
            set -e

            if [ $rc -eq 0 ]; then
                log_success "Cache residue check passed (no leaks)"
                report_test "Final_CacheResidue" "PASS" "0"
            elif [ $rc -eq 1 ]; then
                if [ "$residue_mode" = "fail" ]; then
                    log_error "Cache residue detected (RESIDUE_MODE=fail) — see report above"
                    report_test "Final_CacheResidue" "FAIL" "0" "residue detected"
                    test_failed=true
                else
                    log_warn "Cache residue detected (RESIDUE_MODE=warn — not failing build); see report above"
                    report_test "Final_CacheResidue" "SKIP" "0" "residue detected; RESIDUE_MODE=warn"
                fi
            elif [ $rc -eq 2 ]; then
                log_error "cache_residue infrastructure error (rc=2)"
                report_test "Final_CacheResidue" "FAIL" "0" "infrastructure error rc=2"
                test_failed=true
            else
                log_error "cache_residue unexpected exit code: $rc"
                report_test "Final_CacheResidue" "FAIL" "0" "unexpected rc=${rc}"
                test_failed=true
            fi
        fi
    else
        report_test "Final_CacheResidue" "SKIP" "0" "explicitly disabled"
    fi

    if $test_failed; then
        REPORT_PHASE="failure report finalization"
        finalize_report "FAIL"
        log_error "Some tests failed"
        log_info "View detailed logs: ${TEST_LOG_DIR}"
        # Enumerate failures: each *.log under TEST_LOG_DIR, scan for "✗" lines
        # plus the next non-blank line (typically the failure detail).
        if [ -d "${TEST_LOG_DIR}" ]; then
            local found_any=false
            for log_file in "${TEST_LOG_DIR}"/*.log; do
                [ -f "$log_file" ] || continue
                # Capture failure markers from BOTH suite tests (✗ <name>) and
                # dynamic scenarios (Error: ...). For ✗ lines also pick up the
                # immediately following indented detail line.
                local extracted
                extracted=$(awk '
                    /✗ / {
                        print "    " $0
                        getline next_line
                        if (next_line ~ /^[[:space:]]+[^[:space:]]/) print "      " next_line
                        next
                    }
                    /^Error:/ {
                        # Dynamic-scenario failure: print the line, capped to keep
                        # the summary readable. Full detail still in the log file.
                        line = $0
                        if (length(line) > 600) line = substr(line, 1, 600) "..."
                        print "    " line
                        next
                    }
                ' "$log_file" 2>/dev/null)
                if [ -n "$extracted" ]; then
                    if ! $found_any; then
                        log_error "Failed tests:"
                        found_any=true
                    fi
                    echo "  --- $(basename "${log_file%.log}") ---"
                    echo "$extracted"
                fi
            done
            # Also surface YAML load failures (config rejected before tests ran).
            local load_log="${WORK_DIR}/logs/load_config.log"
            if [ -f "$load_log" ]; then
                local load_failures
                load_failures=$(grep -E "(Loadfailed|Load failed|partialconfigLoadfailed)" "$load_log" 2>/dev/null | head -10)
                if [ -n "$load_failures" ]; then
                    log_error "Config load failures (see ${load_log}):"
                    echo "$load_failures" | sed 's/^/  /'
                fi
            fi
        fi
        exit 1
    fi

    # Completed
    REPORT_PHASE="successful report finalization"
    log_section "Test completed"
    log_success "All tests passed!"
    finalize_report "PASS"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    main "$@"
fi
