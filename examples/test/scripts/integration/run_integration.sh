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

# Required open files limit for integration run.
REQUIRED_NOFILE=65535

# Report file path (will be set after WORK_DIR is determined)
REPORT_FILE=""

# Test counters
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0

# LDAP integration dependency (OpenLDAP via Docker Compose)
LDAP_COMPOSE_DIR=""
LDAP_COMPOSE_FILE=""
LDAP_SERVICE_REQUIRED=false
LDAP_SERVICE_STARTED=false
CONTROLLER_GRPC_PORT=50051

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

# =============================================================================
# Slow test management
# =============================================================================
# Slow tests list (skipped by default, run with --full-test)
SLOW_TESTS=(
    "HTTPRoute_Backend_Timeout"
    "HTTPRoute_Backend_HealthCheckTransition"
    "EdgionPlugins_AllEndpointStatus"
    "EdgionPlugins_LdapAuth"
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
REQUIRED_LO_ALIASES=("127.0.0.2" "127.0.0.3" "127.0.0.4")

ensure_loopback_aliases() {
    if [ "$(uname -s)" != "Darwin" ]; then
        return 0
    fi

    local missing=()
    local lo_inet
    lo_inet=$(ifconfig lo0 2>/dev/null | awk '/inet /{print $2}')
    for ip in "${REQUIRED_LO_ALIASES[@]}"; do
        if ! grep -qx "$ip" <<< "$lo_inet"; then
            missing+=("$ip")
        fi
    done

    if [ "${#missing[@]}" -eq 0 ]; then
        log_success "lo0 aliases present (${REQUIRED_LO_ALIASES[*]})"
        return 0
    fi

    log_error "lo0 is missing required aliases on macOS: ${missing[*]}"
    echo ""
    echo "  HTTPRoute/Backend/LB* tests use 127.0.0.2 / 127.0.0.3 / 127.0.0.4 as"
    echo "  distinct LB endpoints. macOS does not auto-route 127/8 to lo0 like"
    echo "  Linux, so without these aliases connections silently time out."
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

# Verify exactly one edgion-gateway process is owned by the current user.
# Multiple processes mean a violation of the "shared Gateway" principle:
# new suites must reuse the main Gateway started by start_all_with_conf.sh,
# not spawn their own. Independent-topology scripts (test_file_watcher.sh,
# test_resource_abnormal_metrics.sh, run_conf_sync_mtls_test.sh, etc.) run
# outside this entry point and are not subject to this check.
# See skills/05-testing/01-integration-testing.md "Gateway / Controller 复用原则".
check_single_gateway_process() {
    local pids count
    pids=$(pgrep -u "$USER" edgion-gateway 2>/dev/null || true)
    if [ -z "$pids" ]; then
        log_error "No edgion-gateway process found after start. Check start_all_with_conf.sh logs."
        return 1
    fi
    count=$(echo "$pids" | wc -l | tr -d ' ')
    if [ "$count" -gt 1 ]; then
        log_error "Detected ${count} edgion-gateway processes — violates the shared-Gateway principle"
        log_error "Integration suites must reuse the main Gateway. See:"
        log_error "  skills/05-testing/01-integration-testing.md (Gateway / Controller 复用原则)"
        log_error "Offending PIDs:"
        for pid in $pids; do
            ps -o pid=,command= -p "$pid" 2>/dev/null | sed 's/^/  /' >&2 || true
        done
        return 1
    fi
    log_success "Single edgion-gateway process verified (pid=${pids})"
    return 0
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
    
    # Pre-increment everywhere: under `set -e`, post-increment on a counter at 0
    # returns the pre-value (0), which bash treats as failure and aborts the script.
    # Pre-increment returns the post-value (>=1), so set -e never fires.
    ((++TOTAL_TESTS))
    if [ "$result" = "PASS" ]; then
        ((++PASSED_TESTS))
        echo "[PASS] ${name} (${duration}s)" >> "$REPORT_FILE"
    else
        ((++FAILED_TESTS))
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
# Dynamic Scenarios function (scenario-driven replacement of run_dynamic_tests)
# =============================================================================
run_dynamic_scenarios() {
    local test_client="${PROJECT_ROOT}/target/debug/examples/test_client"

    log_section "🔄 Dynamic Scenarios"

    if [ ! -x "$test_client" ]; then
        log_error "test_client binary not found: $test_client"
        return 1
    fi

    local ids
    ids=$("$test_client" --list-dynamic-scenarios 2>/dev/null || true)
    if [ -z "$ids" ]; then
        log_info "No dynamic scenarios registered (--list-dynamic-scenarios returned empty)"
        return 0
    fi

    local scenario
    for scenario in $ids; do
        run_test "Dyn_${scenario}" \
            "${test_client} -g --dynamic-scenario ${scenario}" || test_failed=true
    done

    log_success "Dynamic scenarios finished"
}

# =============================================================================
# Cleanup function
# =============================================================================
cleanup() {
    if $DO_CLEANUP; then
        log_section "Cleanup: Stop all services"
        if $LDAP_SERVICE_STARTED; then
            log_info "Stopping OpenLDAP test service..."
            docker compose -f "${LDAP_COMPOSE_FILE}" down --timeout 5 >/dev/null 2>&1 || true
        fi
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    else
        if $LDAP_SERVICE_STARTED; then
            log_info "OpenLDAP test service still running (keep-alive mode): ${LDAP_COMPOSE_FILE}"
        fi
    fi
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

    LDAP_COMPOSE_DIR="${PROJECT_ROOT}/examples/test/conf/Services/ldap-openldap"
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
# Verify all `role="..."` label values in /metrics are in the expected whitelist.
# Defends against cardinality explosion from typos / new roles introduced unintentionally.
# Returns 0 on success, 1 on bad label found.
# =============================================================================
verify_metrics_labels() {
    local endpoint=$1 component=$2
    local body
    body=$(curl -sf -m 5 "$endpoint" 2>/dev/null)
    if [ -z "$body" ]; then
        log_warn "[$component] /metrics not reachable: $endpoint (skipping label check)"
        return 0
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
            'role="center"'|'role="controller"')
                ;;
            *)
                log_error "[$component] /metrics has unexpected role label: $line (allowed: center, controller)"
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
    echo "  --controller-grpc-port <port>  Override controller/gateway gRPC sync port (default: 50051)"
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
                    if ! should_skip_test "HTTPRoute_Backend_HealthCheckTransition"; then
                        run_test "HTTPRoute_Backend_HealthCheckTransition" "${PROJECT_ROOT}/target/debug/examples/test_client -g health-check-transition" || test_failed=true
                    else
                        log_info "Skipping slow test: HTTPRoute_Backend_HealthCheckTransition (use --full-test to run)"
                    fi
                    run_test "HTTPRoute_Filters" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Filters" || test_failed=true
                    run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Protocol/WebSocket" || test_failed=true
                    run_test "HTTPRoute_Protocol_H2C" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Protocol/H2C" || test_failed=true
                    run_test "HTTPRoute_Protocol_H2TrailerMalformed" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Protocol/H2TrailerMalformed" || test_failed=true
                    run_test "HTTPRoute_MultiPort" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i MultiPort" || test_failed=true
                else
                    # Run specified sub-item test
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "HTTPRoute_${item_safe}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            GRPCRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "GRPCRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Basic" || test_failed=true
                    run_test "GRPCRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Match" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "GRPCRoute_${item_safe}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            TCPRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "TCPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i Basic" || test_failed=true
                    run_test "TCPRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i StreamPlugins" || test_failed=true
                    run_test "TCPRoute_GlobalConnectionIpRestriction" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i GlobalConnectionIpRestriction" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "TCPRoute_${item_safe}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            TLSRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "TLSRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i Basic" || test_failed=true
                    run_test "TLSRoute_ProxyProtocol" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i ProxyProtocol" || test_failed=true
                    run_test "TLSRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i StreamPlugins" || test_failed=true
                    run_test "TLSRoute_MultiSNI" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i MultiSNI" || test_failed=true
                    run_test "TLSRoute_BothAbsentParentRef" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i BothAbsentParentRef" || test_failed=true
                    run_test "TLSRoute_Passthrough" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i Passthrough" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "TLSRoute_${item_safe}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            UDPRoute)
                run_test "UDPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r UDPRoute -i Basic" || test_failed=true
                ;;
            Gateway)
                if [ -z "$G_ITEM" ]; then
                    run_test "Gateway_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Security" || test_failed=true
                    run_test "Gateway_RealIP" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i RealIP" || test_failed=true
                    run_test "Gateway_TLS_BackendTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/BackendTLS" || test_failed=true
                    run_test "Gateway_TLS_GatewayTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/GatewayTLS" || test_failed=true
                    run_test "Gateway_ListenerHostname" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i ListenerHostname" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Same" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Same" || test_failed=true
                    run_test "Gateway_AllowedRoutes_All" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/All" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Kinds" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Kinds" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Selector" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Selector" || test_failed=true
                    run_test "Gateway_Combined" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Combined" || test_failed=true
                    run_test "Gateway_StreamPlugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i StreamPlugins" || test_failed=true
                    run_test "Gateway_TLS_BackendTLSSAN" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/BackendTLSSAN" || test_failed=true
                    run_test "Gateway_TLS_NoHostnameListener" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/NoHostnameListener" || test_failed=true
                    run_test "Gateway_FrontendValidation" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i FrontendValidation" || test_failed=true
                    run_test "Gateway_PortConflict" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i PortConflict" || test_failed=true
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
                    run_test "EdgionPlugins_AllConditions" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PluginCondition/AllConditions" || test_failed=true
                    run_test "EdgionPlugins_CtxSet" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i CtxSet" || test_failed=true
                    run_test "EdgionPlugins_JwtAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i JwtAuth" || test_failed=true
                    run_test "EdgionPlugins_JweDecrypt" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i JweDecrypt" || test_failed=true
                    run_test "EdgionPlugins_HmacAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i HmacAuth" || test_failed=true
                    run_test "EdgionPlugins_HeaderCertAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i HeaderCertAuth" || test_failed=true
                    run_test "EdgionPlugins_KeyAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i KeyAuth" || test_failed=true
                    run_test "EdgionPlugins_BasicAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i BasicAuth" || test_failed=true
                    if ! should_skip_test "EdgionPlugins_LdapAuth"; then
                        run_test "EdgionPlugins_LdapAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i LdapAuth" || test_failed=true
                    else
                        log_info "Skipping complex test: EdgionPlugins_LdapAuth (requires --full-test)"
                    fi
                    run_test "EdgionPlugins_ProxyRewrite" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ProxyRewrite" || test_failed=true
                    run_test "EdgionPlugins_RateLimit" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RateLimit" || test_failed=true
                    run_test "EdgionPlugins_RateLimitRedis" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RateLimitRedis" || test_failed=true
                    run_test "EdgionPlugins_RealIp" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RealIp" || test_failed=true
                    run_test "EdgionPlugins_RequestMirror" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RequestMirror" || test_failed=true
                    run_test "EdgionPlugins_ResponseRewrite" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ResponseRewrite" || test_failed=true
                    run_test "EdgionPlugins_RequestRestriction" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RequestRestriction" || test_failed=true
                    run_test "EdgionPlugins_PathNormalization" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PathNormalization" || test_failed=true
                    run_test "EdgionPlugins_PathNormalization_ForwardView" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PathNormalization/ForwardView" || test_failed=true
                    run_test "EdgionPlugins_ForwardAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ForwardAuth" || test_failed=true
                    run_test "EdgionPlugins_OpenidConnect" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i OpenidConnect" || test_failed=true
                    run_test "EdgionPlugins_BandwidthLimit" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i BandwidthLimit" || test_failed=true
                    run_test "EdgionPlugins_DirectEndpoint" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DirectEndpoint" || test_failed=true
                    run_test "EdgionPlugins_DynamicInternalUpstream" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DynamicInternalUpstream" || test_failed=true
                    run_test "EdgionPlugins_DynamicExternalUpstream" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DynamicExternalUpstream" || test_failed=true
                    run_test "EdgionPlugins_RegionRoute" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RegionRoute" || test_failed=true
                    run_test "EdgionPlugins_GlobalConnectionIpRestriction" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i GlobalConnectionIpRestriction" || test_failed=true
                    run_test "EdgionPlugins_PluginMetaDataStatus" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PluginMetaDataStatus" || test_failed=true
                    run_test "EdgionPlugins_ResolvedRefs" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ResolvedRefs" || test_failed=true
                    run_test "EdgionPlugins_WebhookKeyGet" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i WebhookKeyGet" || test_failed=true
                    run_test "EdgionPlugins_Cors" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i Cors" || test_failed=true
                    run_test "EdgionPlugins_Dsl" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i Dsl" || test_failed=true
                    run_test "EdgionPlugins_DslGrpcError" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DslGrpcError" || test_failed=true
                    if ! should_skip_test "EdgionPlugins_AllEndpointStatus"; then
                        run_test "EdgionPlugins_AllEndpointStatus" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i AllEndpointStatus" || test_failed=true
                    else
                        log_info "Skipping slow test: EdgionPlugins_AllEndpointStatus (use --full-test to run)"
                    fi
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    local test_name="EdgionPlugins_${item_safe}"
                    if ! should_skip_test "$test_name"; then
                        run_test "$test_name" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ${G_ITEM}" || test_failed=true
                    else
                        log_info "Skipping complex test: ${test_name} (requires --full-test)"
                    fi
                fi
                ;;
            EdgionTls)
                if [ -z "$G_ITEM" ]; then
                    run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i https" || test_failed=true
                    run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i grpctls" || test_failed=true
                    run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i mTLS" || test_failed=true
                    run_test "EdgionTls_cipher" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i cipher" || test_failed=true
                    run_test "EdgionTls_fallback_sni" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i fallback_sni" || test_failed=true
                    run_test "EdgionTls_BothAbsentParentRef" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i BothAbsentParentRef" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "EdgionTls_${item_safe}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            Status)
                if [ -z "$G_ITEM" ]; then
                    run_test "Status_RefGrant" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i RefGrant" || test_failed=true
                    run_test "Status_RefGrant_TCPRouteCrossNsSvc" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i RefGrant/TCPRouteCrossNsSvc" || test_failed=true
                    run_test "Status_RefGrant_TLSRouteCrossNsSvc" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i RefGrant/TLSRouteCrossNsSvc" || test_failed=true
                    run_test "Status_BackendTLSPolicy" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i BackendTLSPolicy" || test_failed=true
                    run_test "Status_GatewayFrontendValidation" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i GatewayFrontendValidation" || test_failed=true
                    run_test "Status_TLSListenerMode" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i TLSListenerMode" || test_failed=true
                    run_test "Status_TLSRoute" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i TLSRoute" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "Status_${item_safe}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            "LinkSys")
                if [ -z "$G_ITEM" ]; then
                    run_test "LinkSys_Status" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r LinkSys -i Status" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "LinkSys_${item_safe}" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r LinkSys -i ${G_ITEM}" || test_failed=true
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
        run_test "HTTPRoute_Backend_LBLeastConn" "${PROJECT_ROOT}/target/debug/examples/test_client -g lb-lc" || test_failed=true
        run_test "HTTPRoute_Backend_LBLeastConnRetryOnStatus" "${PROJECT_ROOT}/target/debug/examples/test_client -g lb-lc-rs" || test_failed=true
        run_test "HTTPRoute_Backend_WeightedBackend" "${PROJECT_ROOT}/target/debug/examples/test_client -g weighted-backend" || test_failed=true
        run_test "HTTPRoute_Backend_HealthCheck" "${PROJECT_ROOT}/target/debug/examples/test_client -g health-check" || test_failed=true
        if ! should_skip_test "HTTPRoute_Backend_Timeout"; then
            run_test "HTTPRoute_Backend_Timeout" "${PROJECT_ROOT}/target/debug/examples/test_client -g timeout" || test_failed=true
        else
            log_info "Skipping slow test: HTTPRoute_Backend_Timeout (use --full-test to run)"
        fi
        if ! should_skip_test "HTTPRoute_Backend_HealthCheckTransition"; then
            run_test "HTTPRoute_Backend_HealthCheckTransition" "${PROJECT_ROOT}/target/debug/examples/test_client -g health-check-transition" || test_failed=true
        else
            log_info "Skipping slow test: HTTPRoute_Backend_HealthCheckTransition (use --full-test to run)"
        fi
        run_test "HTTPRoute_Backend_RetryOnStatus" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend/RetryOnStatus" || test_failed=true
        run_test "HTTPRoute_Backend_CircuitBreaker" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend/CircuitBreaker" || test_failed=true
        run_test "HTTPRoute_Backend_SessionPersistence" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend/SessionPersistence" || test_failed=true
        run_test "HTTPRoute_Backend_H2BackendMtlsPersistence" "${PROJECT_ROOT}/target/debug/examples/test_client -g h2-mtls-sp" || test_failed=true
        run_test "HTTPRoute_Backend_BackendTLSSNIHostOverride" "${PROJECT_ROOT}/target/debug/examples/test_client -g btls-sni" || test_failed=true
        run_test "HTTPRoute_Backend_PartiallyInvalid" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend/PartiallyInvalid" || test_failed=true
        run_test "HTTPRoute_Backend_GrpcHealthCheck" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend/GrpcHealthCheck" || test_failed=true
        run_test "HTTPRoute_Backend_DnsResolutionFailure" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Backend/DnsResolutionFailure" || test_failed=true
        run_test "HTTPRoute_Filters_Redirect" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-redirect" || test_failed=true
        run_test "HTTPRoute_Filters_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g http-security" || test_failed=true
        run_test "HTTPRoute_Filters_HeaderModifier" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Filters/HeaderModifier" || test_failed=true
        run_test "HTTPRoute_Filters_FilterConflict" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i Filters/FilterConflict" || test_failed=true
        run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/examples/test_client -g websocket" || test_failed=true
        run_test "HTTPRoute_MultiPort" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r HTTPRoute -i MultiPort" || test_failed=true
        
        # GRPCRoute Tests
        run_test "GRPCRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Basic" || test_failed=true
        run_test "GRPCRoute_Match" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r GRPCRoute -i Match" || test_failed=true
        
        # TCPRoute Tests
        run_test "TCPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i Basic" || test_failed=true
        run_test "TCPRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TCPRoute -i StreamPlugins" || test_failed=true
        
        # TLSRoute Tests
        run_test "TLSRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i Basic" || test_failed=true
        run_test "TLSRoute_ProxyProtocol" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i ProxyProtocol" || test_failed=true
        run_test "TLSRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i StreamPlugins" || test_failed=true
        run_test "TLSRoute_MultiSNI" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i MultiSNI" || test_failed=true
        run_test "TLSRoute_BothAbsentParentRef" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i BothAbsentParentRef" || test_failed=true
        run_test "TLSRoute_Passthrough" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r TLSRoute -i Passthrough" || test_failed=true

        # UDPRoute Tests
        run_test "UDPRoute_Basic" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r UDPRoute -i Basic" || test_failed=true
        
        # Gateway Tests
        run_test "Gateway_Security" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Security" || test_failed=true
        run_test "Gateway_RealIP" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i RealIP" || test_failed=true
        run_test "Gateway_TLS_BackendTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/BackendTLS" || test_failed=true
        # EdgionPlugins Tests
        run_test "EdgionPlugins_DebugAccessLog" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DebugAccessLog" || test_failed=true
        run_test "EdgionPlugins_PluginCondition" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PluginCondition" || test_failed=true
        run_test "EdgionPlugins_AllConditions" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PluginCondition/AllConditions" || test_failed=true
        run_test "EdgionPlugins_CtxSet" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i CtxSet" || test_failed=true
        run_test "EdgionPlugins_JwtAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i JwtAuth" || test_failed=true
        run_test "EdgionPlugins_JweDecrypt" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i JweDecrypt" || test_failed=true
        run_test "EdgionPlugins_HmacAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i HmacAuth" || test_failed=true
        run_test "EdgionPlugins_HeaderCertAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i HeaderCertAuth" || test_failed=true
        run_test "EdgionPlugins_KeyAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i KeyAuth" || test_failed=true
        run_test "EdgionPlugins_BasicAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i BasicAuth" || test_failed=true
        if ! should_skip_test "EdgionPlugins_LdapAuth"; then
            run_test "EdgionPlugins_LdapAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i LdapAuth" || test_failed=true
        else
            log_info "Skipping complex test: EdgionPlugins_LdapAuth (requires --full-test)"
        fi
        run_test "EdgionPlugins_ProxyRewrite" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ProxyRewrite" || test_failed=true
        run_test "EdgionPlugins_RateLimit" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RateLimit" || test_failed=true
        run_test "EdgionPlugins_RateLimitRedis" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RateLimitRedis" || test_failed=true
        run_test "EdgionPlugins_RealIp" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RealIp" || test_failed=true
        run_test "EdgionPlugins_RequestMirror" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RequestMirror" || test_failed=true
        run_test "EdgionPlugins_ResponseRewrite" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ResponseRewrite" || test_failed=true
        run_test "EdgionPlugins_RequestRestriction" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RequestRestriction" || test_failed=true
        run_test "EdgionPlugins_PathNormalization" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PathNormalization" || test_failed=true
        run_test "EdgionPlugins_PathNormalization_ForwardView" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PathNormalization/ForwardView" || test_failed=true
        run_test "EdgionPlugins_ForwardAuth" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ForwardAuth" || test_failed=true
        run_test "EdgionPlugins_OpenidConnect" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i OpenidConnect" || test_failed=true
        run_test "EdgionPlugins_BandwidthLimit" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i BandwidthLimit" || test_failed=true
        run_test "EdgionPlugins_DirectEndpoint" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DirectEndpoint" || test_failed=true
        run_test "EdgionPlugins_DynamicInternalUpstream" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DynamicInternalUpstream" || test_failed=true
        run_test "EdgionPlugins_DynamicExternalUpstream" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DynamicExternalUpstream" || test_failed=true
        run_test "EdgionPlugins_RegionRoute" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i RegionRoute" || test_failed=true
        run_test "EdgionPlugins_GlobalConnectionIpRestriction" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i GlobalConnectionIpRestriction" || test_failed=true
        run_test "EdgionPlugins_PluginMetaDataStatus" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i PluginMetaDataStatus" || test_failed=true
        run_test "EdgionPlugins_ResolvedRefs" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i ResolvedRefs" || test_failed=true
        run_test "EdgionPlugins_WebhookKeyGet" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i WebhookKeyGet" || test_failed=true
        run_test "EdgionPlugins_Dsl" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i Dsl" || test_failed=true
        run_test "EdgionPlugins_DslGrpcError" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i DslGrpcError" || test_failed=true
        if ! should_skip_test "EdgionPlugins_AllEndpointStatus"; then
            run_test "EdgionPlugins_AllEndpointStatus" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i AllEndpointStatus" || test_failed=true
        else
            log_info "Skipping slow test: EdgionPlugins_AllEndpointStatus (use --full-test to run)"
        fi
        run_test "Gateway_TLS_GatewayTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/GatewayTLS" || test_failed=true
        run_test "Gateway_ListenerHostname" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i ListenerHostname" || test_failed=true
        run_test "Gateway_AllowedRoutes_Same" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Same" || test_failed=true
        run_test "Gateway_AllowedRoutes_All" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/All" || test_failed=true
        run_test "Gateway_AllowedRoutes_Kinds" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Kinds" || test_failed=true
        run_test "Gateway_AllowedRoutes_Selector" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i AllowedRoutes/Selector" || test_failed=true
        run_test "Gateway_Combined" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i Combined" || test_failed=true
        run_test "Gateway_StreamPlugins" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i StreamPlugins" || test_failed=true
        run_test "Gateway_TLS_BackendTLSSAN" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/BackendTLSSAN" || test_failed=true
        run_test "Gateway_TLS_NoHostnameListener" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i TLS/NoHostnameListener" || test_failed=true
        run_test "Gateway_FrontendValidation" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i FrontendValidation" || test_failed=true
        run_test "Gateway_PortConflict" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Gateway -i PortConflict" || test_failed=true

        # EdgionTls Tests
        run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i https" || test_failed=true
        run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i grpctls" || test_failed=true
        run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i mTLS" || test_failed=true
        run_test "EdgionTls_cipher" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i cipher" || test_failed=true
        run_test "EdgionTls_fallback_sni" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i fallback_sni" || test_failed=true
        run_test "EdgionTls_BothAbsentParentRef" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionTls -i BothAbsentParentRef" || test_failed=true
        
        # Controller Status Tests
        run_test "Status_RefGrant" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i RefGrant" || test_failed=true
        run_test "Status_RefGrant_TCPRouteCrossNsSvc" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i RefGrant/TCPRouteCrossNsSvc" || test_failed=true
        run_test "Status_RefGrant_TLSRouteCrossNsSvc" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i RefGrant/TLSRouteCrossNsSvc" || test_failed=true
        run_test "Status_BackendTLSPolicy" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i BackendTLSPolicy" || test_failed=true
        run_test "Status_GatewayFrontendValidation" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i GatewayFrontendValidation" || test_failed=true
        run_test "Status_TLSListenerMode" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i TLSListenerMode" || test_failed=true
        run_test "Status_TLSRoute" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r Status -i TLSRoute" || test_failed=true
        run_test "LinkSys_Status" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r LinkSys -i Status" || test_failed=true

        # Controller Resource-Abnormal Metric (standalone shell test; slow, runs its own controller with tick_secs=30)
        if $FULL_TEST; then
            run_test "ResourceAbnormalMetrics" "bash ${SCRIPT_DIR}/test_resource_abnormal_metrics.sh" || test_failed=true
        else
            log_info "Skipping slow test: ResourceAbnormalMetrics (use --full-test to run)"
        fi

        # Dynamic scenarios (always run as part of full test cycle)
        run_dynamic_scenarios
    fi

    # Run dynamic scenarios when narrow scope = Gateway or Status
    if [ -n "$G_RESOURCE" ] && [ "$G_RESOURCE" = "Gateway" ]; then
        run_dynamic_scenarios
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
        log_info "Run 'make coverage-report' after this completes to generate HTML"
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
            --full-test)
                FULL_TEST=true
                shift
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
    
    # Setup cleanup on exit
    trap cleanup EXIT
    
    # Auto-infer suites from resource/item if not explicitly specified
    if [ -z "$suites" ] && [ -n "$G_RESOURCE" ]; then
        # Always include base and HTTPRoute/Basic for dependencies (Service, EndpointSlice, etc.)
        local base_suites="base,HTTPRoute/Basic"
        # EdgionPlugins needs its own base Gateway
        if [ "$G_RESOURCE" = "EdgionPlugins" ]; then
            base_suites="${base_suites},EdgionPlugins/base"
            # When running all EdgionPlugins tests, load all plugin configs
            if [ -z "$G_ITEM" ]; then
                suites="${base_suites},EdgionPlugins/DebugAccessLog,EdgionPlugins/PluginCondition,EdgionPlugins/CtxSet,EdgionPlugins/BasicAuth,EdgionPlugins/JwtAuth,EdgionPlugins/JweDecrypt,EdgionPlugins/HmacAuth,EdgionPlugins/HeaderCertAuth,EdgionPlugins/KeyAuth,EdgionPlugins/ProxyRewrite,EdgionPlugins/RateLimit,EdgionPlugins/RealIp,EdgionPlugins/ResponseRewrite,EdgionPlugins/RequestRestriction,EdgionPlugins/PathNormalization,EdgionPlugins/PathNormalization/ForwardView,EdgionPlugins/ForwardAuth,EdgionPlugins/OpenidConnect,EdgionPlugins/BandwidthLimit,EdgionPlugins/DirectEndpoint,EdgionPlugins/DynamicInternalUpstream,EdgionPlugins/DynamicExternalUpstream,EdgionPlugins/RegionRoute,EdgionPlugins/GlobalConnectionIpRestriction,EdgionPlugins/WebhookKeyGet,EdgionPlugins/Dsl,EdgionPlugins/DslGrpcError,EdgionPlugins/AllEndpointStatus"
                if $FULL_TEST; then
                    suites="${suites},EdgionPlugins/LdapAuth"
                fi
            else
                suites="${base_suites}"
                if [ "${G_RESOURCE}/${G_ITEM}" = "EdgionPlugins/LdapAuth" ] && ! $FULL_TEST; then
                    log_info "LdapAuth suite requires --full-test, config load will be skipped"
                else
                    suites="${suites},${G_RESOURCE}/${G_ITEM}"
                fi
            fi
        elif [ "$G_RESOURCE" = "Status" ]; then
            if [ -n "$G_ITEM" ]; then
                suites="${base_suites},Status/${G_ITEM}"
            else
                suites="${base_suites},Status/RefGrant,Status/BackendTLSPolicy,Status/GatewayFrontendValidation,Status/TLSListenerMode,Status/TLSRoute"
            fi
        elif [ -n "$G_ITEM" ]; then
            if [ "$G_RESOURCE" = "Gateway" ] && [ "$G_ITEM" = "TLS/BackendTLS" ]; then
                suites="${base_suites},HTTPRoute/Backend/BackendTLS"
            elif [ "$G_RESOURCE" = "Gateway" ] && [ "$G_ITEM" = "TLS/BackendTLSSAN" ]; then
                suites="${base_suites},HTTPRoute/Backend/BackendTLSSAN,HTTPRoute/Backend/BackendTLS"
            else
                suites="${base_suites},${G_RESOURCE}/${G_ITEM}"
            fi
        fi
    fi

    # Fast mode guard: LDAP integration test is complex (Docker-required), full-test only.
    if ! $FULL_TEST && [ "$G_RESOURCE" = "EdgionPlugins" ] && [ "$G_ITEM" = "LdapAuth" ]; then
        log_info "EdgionPlugins/LdapAuth is a complex Docker-based test and requires --full-test"
        DO_CLEANUP=false
        exit 0
    fi

    if is_ldap_service_required "$suites"; then
        LDAP_SERVICE_REQUIRED=true
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

    if ! ensure_nofile_limit "$REQUIRED_NOFILE"; then
        log_error "File descriptor limit check failed"
        exit 1
    fi

    if ! ensure_loopback_aliases; then
        log_error "Loopback alias check failed"
        exit 1
    fi

    if ! check_docker_environment_ready; then
        log_error "Docker environment check failed"
        exit 1
    fi

    # Step 0: Start external dependencies when needed
    if ! start_ldap_service_if_needed; then
        log_error "Failed to start external LDAP dependency"
        exit 1
    fi

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

    # Step 1.5: Center no-auth startup (standalone, no Controller/Gateway).
    # Only runs in the default full sweep (no -r / -i targeting). Verifies that
    # when no [local_auth] / [auth] is configured: server starts without panic,
    # business routes return 503 fail-close, /health and /api/v1/auth/status stay
    # reachable, and startup emits the "No authentication configured" WARN.
    if [ -z "$G_RESOURCE" ]; then
        log_section "Center no-auth startup"
        if ! "${SCRIPT_DIR}/test_center_no_auth.sh"; then
            log_error "FAIL: Center no-auth startup"
            exit 1
        fi
        log_success "Center no-auth startup passed"
    fi

    # Step 2: Start services (including config load)
    if $do_start; then
        log_section "Step 2: Start all services and load config"
        export EDGION_TEST_CONTROLLER_GRPC_PORT="${CONTROLLER_GRPC_PORT}"
        
        # Build start command
        local start_cmd="${UTILS_DIR}/start_all_with_conf.sh"
        if [ -n "$suites" ]; then
            start_cmd="$start_cmd --suites $suites"
        fi
        start_cmd="$start_cmd --controller-grpc-port ${CONTROLLER_GRPC_PORT}"
        
        # Execute start script
        # NOTE: Must separate 'local' from command substitution, because
        # 'local' always returns 0 and masks the real exit code of $().
        local output exit_code
        output=$($start_cmd 2>&1) && exit_code=0 || exit_code=$?
        
        # Show output
        echo "$output"
        
        if [ $exit_code -ne 0 ]; then
            log_error "Start failed (exit code: $exit_code)"
            exit 1
        fi
        
        # Get work directory from .current file (written by start_all_with_conf.sh)
        # This is more robust than 'tail -1' which can be polluted by background
        # process output leaking into the captured stdout.
        local current_file="${PROJECT_ROOT}/integration_testing/.current"
        if [ -f "$current_file" ]; then
            WORK_DIR=$(cat "$current_file")
        else
            log_error "Work directory file not found: $current_file"
            exit 1
        fi
        
        # Set environment variables
        export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
        export EDGION_WORK_DIR="${WORK_DIR}"
        
        log_success "All services started, config loaded"
        log_info "Work directory: ${WORK_DIR}"
    else
        log_info "Skip start step"
        if [ -f "${PROJECT_ROOT}/integration_testing/.current" ]; then
            WORK_DIR=$(cat "${PROJECT_ROOT}/integration_testing/.current")
            export EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/edgion_access.log"
            export EDGION_WORK_DIR="${WORK_DIR}"
            log_info "Using existing work directory: ${WORK_DIR}"
        fi
    fi

    # Enforce shared-Gateway principle: exactly one edgion-gateway process.
    if ! check_single_gateway_process; then
        exit 1
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

    # =============================================================================
    # Final Step: Metrics Label Whitelist
    # =============================================================================
    log_section "Final Step: Metrics Label Whitelist"
    if ! verify_metrics_labels "http://127.0.0.1:5800/metrics" "controller"; then
        log_error "Metrics label validation FAILED"
        test_failed=true
    fi

    # =============================================================================
    # Final Step: Cache Residue Detection (leak detection across all scenarios)
    # =============================================================================
    if [ "$SKIP_RESIDUE_CHECK" != "true" ]; then
        log_section "Final Step: Cache Residue Detection"

        local residue_bin="${PROJECT_ROOT}/target/debug/cache_residue"
        if [ ! -x "$residue_bin" ]; then
            log_warn "cache_residue binary not built; building now..."
            (cd "$PROJECT_ROOT" && cargo build --bin cache_residue) || {
                log_error "cache_residue build failed — skipping leak check"
                # do not fail the run on missing binary; fall through
                residue_bin=""
            }
        fi

        if [ -n "$residue_bin" ] && [ -x "$residue_bin" ]; then
            # Always invoke validator with --mode fail so exit 1 unambiguously means
            # "residue detected". Shell-side RESIDUE_MODE then decides whether to
            # block the build (default: fail — opt out with RESIDUE_MODE=warn).
            local residue_mode="${RESIDUE_MODE:-fail}"
            "$residue_bin" \
                --controller-addr "http://127.0.0.1:5800" \
                --gateway-addr    "http://127.0.0.1:5900" \
                --timeout-secs 30 \
                --mode fail
            local rc=$?

            if [ $rc -eq 0 ]; then
                log_success "Cache residue check passed (no leaks)"
            elif [ $rc -eq 1 ]; then
                if [ "$residue_mode" = "fail" ]; then
                    log_error "Cache residue detected (RESIDUE_MODE=fail) — see report above"
                    ((++FAILED_TESTS))
                    test_failed=true
                else
                    log_warn "Cache residue detected (RESIDUE_MODE=warn — not failing build); see report above"
                fi
            elif [ $rc -eq 2 ]; then
                log_error "cache_residue infrastructure error (rc=2)"
                ((++FAILED_TESTS))
                test_failed=true
            else
                log_error "cache_residue unexpected exit code: $rc"
                ((++FAILED_TESTS))
                test_failed=true
            fi
        fi
    fi

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
