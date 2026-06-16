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

# Redis integration dependency (used by EdgionPlugins/Dsl, EdgionPlugins/RateLimitRedis,
# and any LinkSys Redis test driven by the main runner). The bundled docker-compose
# matches edgion-tests/integration/conf/LinkSys/Redis (host 127.0.0.1, port 16379, password
# edgion-test-pwd). When the runner starts Redis itself, cleanup() stops it; an
# externally-managed Redis is left untouched.
REDIS_COMPOSE_DIR=""
REDIS_COMPOSE_FILE=""
REDIS_SERVICE_REQUIRED=false
REDIS_SERVICE_STARTED=false
REDIS_HOST="127.0.0.1"
REDIS_PORT=16379
REDIS_PASSWORD="edgion-test-pwd"
REDIS_CONTAINER_NAME="edgion-test-redis"

CONTROLLER_GRPC_PORT=12151

# Gateway admin bearer token. The gateway admin API (incl. the /api/v1/testing/*
# access-log endpoints) is auth-guarded, so test_client's AccessLogClient must
# present this token. start_all_with_conf.sh defaults the same value, but it runs
# in a command-substitution subshell, so its export does not reach this process or
# the test_client children launched here — export it here so the token is shared
# across the start script, the bash admin helpers, and test_client. Honors an
# operator-provided value.
export EDGION_GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-integration-gateway-admin-token-0123456789}"

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
# See skills/05-testing/01-integration-testing.md "Gateway / Controller reuse principle".
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
        log_error "  skills/05-testing/01-integration-testing.md (Gateway / Controller reuse principle)"
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
    local test_client="${PROJECT_ROOT}/target/debug/test_client"

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
        if $REDIS_SERVICE_STARTED; then
            log_info "Stopping Redis test service..."
            docker compose -f "${REDIS_COMPOSE_FILE}" down --timeout 5 >/dev/null 2>&1 || true
        fi
        "${UTILS_DIR}/kill_all.sh" 2>&1 || true
    else
        if $LDAP_SERVICE_STARTED; then
            log_info "OpenLDAP test service still running (keep-alive mode): ${LDAP_COMPOSE_FILE}"
        fi
        if $REDIS_SERVICE_STARTED; then
            log_info "Redis test service still running (keep-alive mode): ${REDIS_COMPOSE_FILE}"
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
# Redis integration dependency
# =============================================================================
# Triggered by EdgionPlugins/Dsl, EdgionPlugins/RateLimitRedis, or the default
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

    if [ "$G_RESOURCE" = "EdgionPlugins" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "Dsl" ] || [ "$G_ITEM" = "RateLimitRedis" ]; then
            return 0
        fi
    fi

    if [ "$G_RESOURCE" = "LinkSys" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "Redis" ]; then
            return 0
        fi
    fi

    if [ "$G_RESOURCE" = "TCPRoute" ]; then
        if [ -z "$G_ITEM" ] || [ "$G_ITEM" = "StreamPlugins" ]; then
            return 0
        fi
    fi

    case ",${explicit_suites}," in
        *,EdgionPlugins/Dsl,*|*,EdgionPlugins/RateLimitRedis,*|*,LinkSys/Redis,*|*,TCPRoute/StreamPlugins,*)
            return 0
            ;;
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

print_redis_install_hints() {
    cat <<EOF

Edgion integration tests require a Redis instance reachable at
  ${REDIS_HOST}:${REDIS_PORT}  (password: ${REDIS_PASSWORD})

This matches the LinkSys fixture in edgion-tests/integration/conf/LinkSys/Redis
(consumed by EdgionPlugins/Dsl, EdgionPlugins/RateLimitRedis, and the
LinkSys Redis suite).

Recommended (any platform with Docker):
  cd ${REDIS_COMPOSE_DIR}
  docker compose up -d
  # the runner will detect and reuse it on the next invocation

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

    log_section "Redis dependency: ensure Redis is available"

    if probe_redis_ready "$REDIS_HOST" "$REDIS_PORT" "$REDIS_PASSWORD"; then
        log_success "Redis already reachable at ${REDIS_HOST}:${REDIS_PORT} (reusing it)"
        return 0
    fi

    log_info "Redis not reachable at ${REDIS_HOST}:${REDIS_PORT} — attempting to start it"

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

    local compose_output=""
    compose_output=$(docker compose -f "${REDIS_COMPOSE_FILE}" up -d 2>&1) || {
        log_error "Failed to start Redis via docker compose"
        echo "$compose_output"
        print_redis_install_hints
        return 1
    }

    local elapsed=0
    local timeout=30
    while [ $elapsed -lt $timeout ]; do
        if probe_redis_ready "$REDIS_HOST" "$REDIS_PORT" "$REDIS_PASSWORD"; then
            REDIS_SERVICE_STARTED=true
            log_success "Redis is ready (started via docker, waited ${elapsed}s)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    log_error "Redis container did not become ready within ${timeout}s"
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
    echo "  --full-test            Run all tests including slow tests (timeout, etc.)"
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
                    run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Basic" || test_failed=true
                    run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Match" || test_failed=true
                    run_test "HTTPRoute_Backend" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Backend" || test_failed=true
                    if ! should_skip_test "HTTPRoute_Backend_HealthCheckTransition"; then
                        run_test "HTTPRoute_Backend_HealthCheckTransition" "${PROJECT_ROOT}/target/debug/test_client -g health-check-transition" || test_failed=true
                    else
                        log_info "Skipping slow test: HTTPRoute_Backend_HealthCheckTransition (use --full-test to run)"
                    fi
                    run_test "HTTPRoute_Filters" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Filters" || test_failed=true
                    run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Protocol/WebSocket" || test_failed=true
                    run_test "HTTPRoute_Protocol_H2C" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Protocol/H2C" || test_failed=true
                    run_test "HTTPRoute_Protocol_GrpcWeb" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Protocol/GrpcWeb" || test_failed=true
                    run_test "HTTPRoute_Protocol_H2TrailerMalformed" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Protocol/H2TrailerMalformed" || test_failed=true
                    run_test "HTTPRoute_MultiPort" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i MultiPort" || test_failed=true
                else
                    # Run specified sub-item test
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "HTTPRoute_${item_safe}" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            GRPCRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "GRPCRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g -r GRPCRoute -i Basic" || test_failed=true
                    run_test "GRPCRoute_Match" "${PROJECT_ROOT}/target/debug/test_client -g -r GRPCRoute -i Match" || test_failed=true
                    run_test "GRPCRoute_RateLimit" "${PROJECT_ROOT}/target/debug/test_client -g -r GRPCRoute -i RateLimit" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "GRPCRoute_${item_safe}" "${PROJECT_ROOT}/target/debug/test_client -g -r GRPCRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            TCPRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "TCPRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g -r TCPRoute -i Basic" || test_failed=true
                    run_test "TCPRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/test_client -g -r TCPRoute -i StreamPlugins" || test_failed=true
                    run_test "TCPRoute_GlobalConnectionIpRestriction" "${PROJECT_ROOT}/target/debug/test_client -g -r TCPRoute -i GlobalConnectionIpRestriction" || test_failed=true
                    run_test "TCPRoute_IdleTimeout" "${PROJECT_ROOT}/target/debug/test_client -g -r TCPRoute -i IdleTimeout" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "TCPRoute_${item_safe}" "${PROJECT_ROOT}/target/debug/test_client -g -r TCPRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            TLSRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "TLSRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i Basic" || test_failed=true
                    run_test "TLSRoute_ProxyProtocol" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i ProxyProtocol" || test_failed=true
                    run_test "TLSRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i StreamPlugins" || test_failed=true
                    run_test "TLSRoute_MultiSNI" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i MultiSNI" || test_failed=true
                    run_test "TLSRoute_BothAbsentParentRef" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i BothAbsentParentRef" || test_failed=true
                    run_test "TLSRoute_Passthrough" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i Passthrough" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "TLSRoute_${item_safe}" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            UDPRoute)
                if [ -z "$G_ITEM" ]; then
                    run_test "UDPRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g -r UDPRoute -i Basic" || test_failed=true
                    run_test "UDPRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/test_client -g -r UDPRoute -i StreamPlugins" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "UDPRoute_${item_safe}" "${PROJECT_ROOT}/target/debug/test_client -g -r UDPRoute -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            Gateway)
                if [ -z "$G_ITEM" ]; then
                    run_test "Gateway_Security" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i Security" || test_failed=true
                    run_test "Gateway_RealIP" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i RealIP" || test_failed=true
                    run_test "Gateway_TLS_BackendTLS" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i TLS/BackendTLS" || test_failed=true
                    run_test "Gateway_TLS_GatewayTLS" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i TLS/GatewayTLS" || test_failed=true
                    run_test "Gateway_ListenerHostname" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i ListenerHostname" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Same" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i AllowedRoutes/Same" || test_failed=true
                    run_test "Gateway_AllowedRoutes_All" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i AllowedRoutes/All" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Kinds" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i AllowedRoutes/Kinds" || test_failed=true
                    run_test "Gateway_AllowedRoutes_Selector" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i AllowedRoutes/Selector" || test_failed=true
                    run_test "Gateway_Combined" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i Combined" || test_failed=true
                    run_test "Gateway_StreamPlugins" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i StreamPlugins" || test_failed=true
                    run_test "Gateway_TLS_BackendTLSSAN" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i TLS/BackendTLSSAN" || test_failed=true
                    run_test "Gateway_TLS_NoHostnameListener" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i TLS/NoHostnameListener" || test_failed=true
                    run_test "Gateway_FrontendValidation" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i FrontendValidation" || test_failed=true
                    run_test "Gateway_PortConflict" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i PortConflict" || test_failed=true
                else
                    # Replace / with _ in item name for log file
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "Gateway_${item_safe}" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            EdgionPlugins)
                if [ -z "$G_ITEM" ]; then
                    run_test "EdgionPlugins_DebugAccessLog" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DebugAccessLog" || test_failed=true
                    run_test "EdgionPlugins_Csrf" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i Csrf" || test_failed=true
                    run_test "EdgionPlugins_PluginCondition" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PluginCondition" || test_failed=true
                    run_test "EdgionPlugins_AllConditions" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PluginCondition/AllConditions" || test_failed=true
                    run_test "EdgionPlugins_CtxSet" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i CtxSet" || test_failed=true
                    run_test "EdgionPlugins_JwtAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i JwtAuth" || test_failed=true
                    run_test "EdgionPlugins_JweDecrypt" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i JweDecrypt" || test_failed=true
                    run_test "EdgionPlugins_HmacAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i HmacAuth" || test_failed=true
                    run_test "EdgionPlugins_HeaderCertAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i HeaderCertAuth" || test_failed=true
                    run_test "EdgionPlugins_KeyAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i KeyAuth" || test_failed=true
                    run_test "EdgionPlugins_BasicAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i BasicAuth" || test_failed=true
                    if ! should_skip_test "EdgionPlugins_LdapAuth"; then
                        run_test "EdgionPlugins_LdapAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i LdapAuth" || test_failed=true
                    else
                        log_info "Skipping complex test: EdgionPlugins_LdapAuth (requires --full-test)"
                    fi
                    run_test "EdgionPlugins_ProxyRewrite" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ProxyRewrite" || test_failed=true
                    run_test "EdgionPlugins_RateLimit" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RateLimit" || test_failed=true
                    run_test "EdgionPlugins_RateLimitRedis" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RateLimitRedis" || test_failed=true
                    run_test "EdgionPlugins_RealIp" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RealIp" || test_failed=true
                    run_test "EdgionPlugins_RequestMirror" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RequestMirror" || test_failed=true
                    run_test "EdgionPlugins_ResponseRewrite" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ResponseRewrite" || test_failed=true
                    run_test "EdgionPlugins_RequestRestriction" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RequestRestriction" || test_failed=true
                    run_test "EdgionPlugins_IpRestriction" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i IpRestriction" || test_failed=true
                    run_test "EdgionPlugins_PathNormalization" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PathNormalization" || test_failed=true
                    run_test "EdgionPlugins_PathNormalization_ForwardView" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PathNormalization/ForwardView" || test_failed=true
                    run_test "EdgionPlugins_ForwardAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ForwardAuth" || test_failed=true
                    run_test "EdgionPlugins_ExtProc" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ExtProc" || test_failed=true
                    run_test "EdgionPlugins_OpenidConnect" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i OpenidConnect" || test_failed=true
                    run_test "EdgionPlugins_BandwidthLimit" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i BandwidthLimit" || test_failed=true
                    run_test "EdgionPlugins_DirectEndpoint" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DirectEndpoint" || test_failed=true
                    run_test "EdgionPlugins_DynamicInternalUpstream" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DynamicInternalUpstream" || test_failed=true
                    run_test "EdgionPlugins_DynamicExternalUpstream" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DynamicExternalUpstream" || test_failed=true
                    run_test "EdgionPlugins_RegionRoute" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RegionRoute" || test_failed=true
                    run_test "EdgionPlugins_GlobalConnectionIpRestriction" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i GlobalConnectionIpRestriction" || test_failed=true
                    run_test "EdgionPlugins_PluginMetaDataStatus" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PluginMetaDataStatus" || test_failed=true
                    run_test "EdgionPlugins_ResolvedRefs" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ResolvedRefs" || test_failed=true
                    run_test "EdgionPlugins_WebhookKeyGet" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i WebhookKeyGet" || test_failed=true
                    run_test "EdgionPlugins_Cors" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i Cors" || test_failed=true
                    run_test "EdgionPlugins_Dsl" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i Dsl" || test_failed=true
                    run_test "EdgionPlugins_DslGrpcError" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DslGrpcError" || test_failed=true
                    if ! should_skip_test "EdgionPlugins_AllEndpointStatus"; then
                        run_test "EdgionPlugins_AllEndpointStatus" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i AllEndpointStatus" || test_failed=true
                    else
                        log_info "Skipping slow test: EdgionPlugins_AllEndpointStatus (use --full-test to run)"
                    fi
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    local test_name="EdgionPlugins_${item_safe}"
                    if ! should_skip_test "$test_name"; then
                        run_test "$test_name" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ${G_ITEM}" || test_failed=true
                    else
                        log_info "Skipping complex test: ${test_name} (requires --full-test)"
                    fi
                fi
                ;;
            EdgionTls)
                if [ -z "$G_ITEM" ]; then
                    run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i https" || test_failed=true
                    run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i grpctls" || test_failed=true
                    run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i mTLS" || test_failed=true
                    run_test "EdgionTls_cipher" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i cipher" || test_failed=true
                    run_test "EdgionTls_mtls_failclose" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i mtls_failclose" || test_failed=true
                    run_test "EdgionTls_fallback_sni" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i fallback_sni" || test_failed=true
                    run_test "EdgionTls_BothAbsentParentRef" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i BothAbsentParentRef" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "EdgionTls_${item_safe}" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            Status)
                if [ -z "$G_ITEM" ]; then
                    run_test "Status_RefGrant" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i RefGrant" || test_failed=true
                    run_test "Status_RefGrant_TCPRouteCrossNsSvc" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i RefGrant/TCPRouteCrossNsSvc" || test_failed=true
                    run_test "Status_RefGrant_TLSRouteCrossNsSvc" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i RefGrant/TLSRouteCrossNsSvc" || test_failed=true
                    run_test "Status_BackendTLSPolicy" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i BackendTLSPolicy" || test_failed=true
                    run_test "Status_GatewayFrontendValidation" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i GatewayFrontendValidation" || test_failed=true
                    run_test "Status_TLSListenerMode" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i TLSListenerMode" || test_failed=true
                    run_test "Status_TLSRoute" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i TLSRoute" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "Status_${item_safe}" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i ${G_ITEM}" || test_failed=true
                fi
                ;;
            "LinkSys")
                if [ -z "$G_ITEM" ]; then
                    run_test "LinkSys_Status" "${PROJECT_ROOT}/target/debug/test_client -g -r LinkSys -i Status" || test_failed=true
                    run_test "LinkSys_Redis" "$(linksys_redis_cmd)" || test_failed=true
                elif [ "$G_ITEM" = "Redis" ]; then
                    run_test "LinkSys_Redis" "$(linksys_redis_cmd)" || test_failed=true
                else
                    local item_safe=$(echo "$G_ITEM" | tr '/' '_')
                    run_test "LinkSys_${item_safe}" "${PROJECT_ROOT}/target/debug/test_client -g -r LinkSys -i ${G_ITEM}" || test_failed=true
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
        
        run_test "http_direct" "${PROJECT_ROOT}/target/debug/test_client http" || test_failed=true
        run_test "grpc_direct" "${PROJECT_ROOT}/target/debug/test_client grpc" || test_failed=true
        run_test "websocket_direct" "${PROJECT_ROOT}/target/debug/test_client websocket" || test_failed=true
        run_test "tcp_direct" "${PROJECT_ROOT}/target/debug/test_client tcp" || test_failed=true
        run_test "udp_direct" "${PROJECT_ROOT}/target/debug/test_client udp" || test_failed=true
        
        if $test_failed; then
            log_error "Direct mode tests failed"
            return 1
        fi
        
        log_section "Run Gateway mode tests"
        
        # HTTPRoute Tests
        run_test "HTTPRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g http" || test_failed=true
        run_test "HTTPRoute_Match" "${PROJECT_ROOT}/target/debug/test_client -g http-match" || test_failed=true
        run_test "HTTPRoute_Backend_LBRoundRobin" "${PROJECT_ROOT}/target/debug/test_client -g lb-rr" || test_failed=true
        run_test "HTTPRoute_Backend_LBConsistentHash" "${PROJECT_ROOT}/target/debug/test_client -g lb-ch" || test_failed=true
        run_test "HTTPRoute_Backend_LBLeastConn" "${PROJECT_ROOT}/target/debug/test_client -g lb-lc" || test_failed=true
        run_test "HTTPRoute_Backend_LBLeastConnRetryOnStatus" "${PROJECT_ROOT}/target/debug/test_client -g lb-lc-rs" || test_failed=true
        run_test "HTTPRoute_Backend_WeightedBackend" "${PROJECT_ROOT}/target/debug/test_client -g weighted-backend" || test_failed=true
        run_test "HTTPRoute_Backend_HealthCheck" "${PROJECT_ROOT}/target/debug/test_client -g health-check" || test_failed=true
        if ! should_skip_test "HTTPRoute_Backend_Timeout"; then
            run_test "HTTPRoute_Backend_Timeout" "${PROJECT_ROOT}/target/debug/test_client -g timeout" || test_failed=true
        else
            log_info "Skipping slow test: HTTPRoute_Backend_Timeout (use --full-test to run)"
        fi
        if ! should_skip_test "HTTPRoute_Backend_HealthCheckTransition"; then
            run_test "HTTPRoute_Backend_HealthCheckTransition" "${PROJECT_ROOT}/target/debug/test_client -g health-check-transition" || test_failed=true
        else
            log_info "Skipping slow test: HTTPRoute_Backend_HealthCheckTransition (use --full-test to run)"
        fi
        run_test "HTTPRoute_Backend_RetryOnStatus" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Backend/RetryOnStatus" || test_failed=true
        if ! should_skip_test "HTTPRoute_Backend_PerTryTimeout"; then
            run_test "HTTPRoute_Backend_PerTryTimeout" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Backend/PerTryTimeout" || test_failed=true
        else
            log_info "Skipping slow test: HTTPRoute_Backend_PerTryTimeout (use --full-test to run)"
        fi
        run_test "HTTPRoute_Backend_CircuitBreaker" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Backend/CircuitBreaker" || test_failed=true
        run_test "HTTPRoute_Backend_SessionPersistence" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Backend/SessionPersistence" || test_failed=true
        run_test "HTTPRoute_Backend_H2BackendMtlsPersistence" "${PROJECT_ROOT}/target/debug/test_client -g h2-mtls-sp" || test_failed=true
        run_test "HTTPRoute_Backend_BackendTLSSNIHostOverride" "${PROJECT_ROOT}/target/debug/test_client -g btls-sni" || test_failed=true
        run_test "HTTPRoute_Backend_PartiallyInvalid" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Backend/PartiallyInvalid" || test_failed=true
        run_test "HTTPRoute_Backend_GrpcHealthCheck" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Backend/GrpcHealthCheck" || test_failed=true
        run_test "HTTPRoute_Backend_DnsResolutionFailure" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Backend/DnsResolutionFailure" || test_failed=true
        run_test "HTTPRoute_Filters_Redirect" "${PROJECT_ROOT}/target/debug/test_client -g http-redirect" || test_failed=true
        run_test "HTTPRoute_Filters_Security" "${PROJECT_ROOT}/target/debug/test_client -g http-security" || test_failed=true
        run_test "HTTPRoute_Filters_HeaderModifier" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Filters/HeaderModifier" || test_failed=true
        run_test "HTTPRoute_Filters_FilterConflict" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i Filters/FilterConflict" || test_failed=true
        run_test "HTTPRoute_Protocol_WebSocket" "${PROJECT_ROOT}/target/debug/test_client -g websocket" || test_failed=true
        run_test "HTTPRoute_MultiPort" "${PROJECT_ROOT}/target/debug/test_client -g -r HTTPRoute -i MultiPort" || test_failed=true
        
        # GRPCRoute Tests
        run_test "GRPCRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g -r GRPCRoute -i Basic" || test_failed=true
        run_test "GRPCRoute_Match" "${PROJECT_ROOT}/target/debug/test_client -g -r GRPCRoute -i Match" || test_failed=true
        run_test "GRPCRoute_RateLimit" "${PROJECT_ROOT}/target/debug/test_client -g -r GRPCRoute -i RateLimit" || test_failed=true
        
        # TCPRoute Tests
        run_test "TCPRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g -r TCPRoute -i Basic" || test_failed=true
        run_test "TCPRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/test_client -g -r TCPRoute -i StreamPlugins" || test_failed=true
        run_test "TCPRoute_IdleTimeout" "${PROJECT_ROOT}/target/debug/test_client -g -r TCPRoute -i IdleTimeout" || test_failed=true
        
        # TLSRoute Tests
        run_test "TLSRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i Basic" || test_failed=true
        run_test "TLSRoute_ProxyProtocol" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i ProxyProtocol" || test_failed=true
        run_test "TLSRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i StreamPlugins" || test_failed=true
        run_test "TLSRoute_MultiSNI" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i MultiSNI" || test_failed=true
        run_test "TLSRoute_BothAbsentParentRef" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i BothAbsentParentRef" || test_failed=true
        run_test "TLSRoute_Passthrough" "${PROJECT_ROOT}/target/debug/test_client -g -r TLSRoute -i Passthrough" || test_failed=true

        # UDPRoute Tests
        run_test "UDPRoute_Basic" "${PROJECT_ROOT}/target/debug/test_client -g -r UDPRoute -i Basic" || test_failed=true
        run_test "UDPRoute_StreamPlugins" "${PROJECT_ROOT}/target/debug/test_client -g -r UDPRoute -i StreamPlugins" || test_failed=true
        
        # Gateway Tests
        run_test "Gateway_Security" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i Security" || test_failed=true
        run_test "Gateway_RealIP" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i RealIP" || test_failed=true
        run_test "Gateway_TLS_BackendTLS" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i TLS/BackendTLS" || test_failed=true
        # EdgionPlugins Tests
        run_test "EdgionPlugins_DebugAccessLog" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DebugAccessLog" || test_failed=true
        run_test "EdgionPlugins_Csrf" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i Csrf" || test_failed=true
        run_test "EdgionPlugins_PluginCondition" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PluginCondition" || test_failed=true
        run_test "EdgionPlugins_AllConditions" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PluginCondition/AllConditions" || test_failed=true
        run_test "EdgionPlugins_CtxSet" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i CtxSet" || test_failed=true
        run_test "EdgionPlugins_JwtAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i JwtAuth" || test_failed=true
        run_test "EdgionPlugins_JweDecrypt" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i JweDecrypt" || test_failed=true
        run_test "EdgionPlugins_HmacAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i HmacAuth" || test_failed=true
        run_test "EdgionPlugins_HeaderCertAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i HeaderCertAuth" || test_failed=true
        run_test "EdgionPlugins_KeyAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i KeyAuth" || test_failed=true
        run_test "EdgionPlugins_BasicAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i BasicAuth" || test_failed=true
        if ! should_skip_test "EdgionPlugins_LdapAuth"; then
            run_test "EdgionPlugins_LdapAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i LdapAuth" || test_failed=true
        else
            log_info "Skipping complex test: EdgionPlugins_LdapAuth (requires --full-test)"
        fi
        run_test "EdgionPlugins_ProxyRewrite" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ProxyRewrite" || test_failed=true
        run_test "EdgionPlugins_RateLimit" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RateLimit" || test_failed=true
        run_test "EdgionPlugins_RateLimitRedis" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RateLimitRedis" || test_failed=true
        run_test "EdgionPlugins_RealIp" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RealIp" || test_failed=true
        run_test "EdgionPlugins_RequestMirror" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RequestMirror" || test_failed=true
        run_test "EdgionPlugins_ResponseRewrite" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ResponseRewrite" || test_failed=true
        run_test "EdgionPlugins_RequestRestriction" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RequestRestriction" || test_failed=true
        run_test "EdgionPlugins_IpRestriction" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i IpRestriction" || test_failed=true
        run_test "EdgionPlugins_PathNormalization" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PathNormalization" || test_failed=true
        run_test "EdgionPlugins_PathNormalization_ForwardView" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PathNormalization/ForwardView" || test_failed=true
        run_test "EdgionPlugins_ForwardAuth" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ForwardAuth" || test_failed=true
        run_test "EdgionPlugins_ExtProc" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ExtProc" || test_failed=true
        run_test "EdgionPlugins_OpenidConnect" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i OpenidConnect" || test_failed=true
        run_test "EdgionPlugins_BandwidthLimit" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i BandwidthLimit" || test_failed=true
        run_test "EdgionPlugins_DirectEndpoint" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DirectEndpoint" || test_failed=true
        run_test "EdgionPlugins_DynamicInternalUpstream" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DynamicInternalUpstream" || test_failed=true
        run_test "EdgionPlugins_DynamicExternalUpstream" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DynamicExternalUpstream" || test_failed=true
        run_test "EdgionPlugins_RegionRoute" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i RegionRoute" || test_failed=true
        run_test "EdgionPlugins_GlobalConnectionIpRestriction" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i GlobalConnectionIpRestriction" || test_failed=true
        run_test "EdgionPlugins_PluginMetaDataStatus" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i PluginMetaDataStatus" || test_failed=true
        run_test "EdgionPlugins_ResolvedRefs" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i ResolvedRefs" || test_failed=true
        run_test "EdgionPlugins_WebhookKeyGet" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i WebhookKeyGet" || test_failed=true
        run_test "EdgionPlugins_Dsl" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i Dsl" || test_failed=true
        run_test "EdgionPlugins_DslGrpcError" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i DslGrpcError" || test_failed=true
        if ! should_skip_test "EdgionPlugins_AllEndpointStatus"; then
            run_test "EdgionPlugins_AllEndpointStatus" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionPlugins -i AllEndpointStatus" || test_failed=true
        else
            log_info "Skipping slow test: EdgionPlugins_AllEndpointStatus (use --full-test to run)"
        fi
        run_test "Gateway_TLS_GatewayTLS" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i TLS/GatewayTLS" || test_failed=true
        run_test "Gateway_ListenerHostname" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i ListenerHostname" || test_failed=true
        run_test "Gateway_AllowedRoutes_Same" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i AllowedRoutes/Same" || test_failed=true
        run_test "Gateway_AllowedRoutes_All" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i AllowedRoutes/All" || test_failed=true
        run_test "Gateway_AllowedRoutes_Kinds" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i AllowedRoutes/Kinds" || test_failed=true
        run_test "Gateway_AllowedRoutes_Selector" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i AllowedRoutes/Selector" || test_failed=true
        run_test "Gateway_Combined" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i Combined" || test_failed=true
        run_test "Gateway_StreamPlugins" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i StreamPlugins" || test_failed=true
        run_test "Gateway_TLS_BackendTLSSAN" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i TLS/BackendTLSSAN" || test_failed=true
        run_test "Gateway_TLS_NoHostnameListener" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i TLS/NoHostnameListener" || test_failed=true
        run_test "Gateway_FrontendValidation" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i FrontendValidation" || test_failed=true
        run_test "Gateway_PortConflict" "${PROJECT_ROOT}/target/debug/test_client -g -r Gateway -i PortConflict" || test_failed=true

        # EdgionTls Tests
        run_test "EdgionTls_https" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i https" || test_failed=true
        run_test "EdgionTls_grpctls" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i grpctls" || test_failed=true
        run_test "EdgionTls_mTLS" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i mTLS" || test_failed=true
        run_test "EdgionTls_cipher" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i cipher" || test_failed=true
        run_test "EdgionTls_mtls_failclose" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i mtls_failclose" || test_failed=true
        run_test "EdgionTls_fallback_sni" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i fallback_sni" || test_failed=true
        run_test "EdgionTls_BothAbsentParentRef" "${PROJECT_ROOT}/target/debug/test_client -g -r EdgionTls -i BothAbsentParentRef" || test_failed=true
        
        # Controller Status Tests
        run_test "Status_RefGrant" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i RefGrant" || test_failed=true
        run_test "Status_RefGrant_TCPRouteCrossNsSvc" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i RefGrant/TCPRouteCrossNsSvc" || test_failed=true
        run_test "Status_RefGrant_TLSRouteCrossNsSvc" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i RefGrant/TLSRouteCrossNsSvc" || test_failed=true
        run_test "Status_BackendTLSPolicy" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i BackendTLSPolicy" || test_failed=true
        run_test "Status_GatewayFrontendValidation" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i GatewayFrontendValidation" || test_failed=true
        run_test "Status_TLSListenerMode" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i TLSListenerMode" || test_failed=true
        run_test "Status_TLSRoute" "${PROJECT_ROOT}/target/debug/test_client -g -r Status -i TLSRoute" || test_failed=true
        run_test "LinkSys_Status" "${PROJECT_ROOT}/target/debug/test_client -g -r LinkSys -i Status" || test_failed=true
        run_test "LinkSys_Redis" "$(linksys_redis_cmd)" || test_failed=true

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
                suites="${base_suites},EdgionPlugins/DebugAccessLog,EdgionPlugins/PluginCondition,EdgionPlugins/CtxSet,EdgionPlugins/BasicAuth,EdgionPlugins/JwtAuth,EdgionPlugins/JweDecrypt,EdgionPlugins/HmacAuth,EdgionPlugins/HeaderCertAuth,EdgionPlugins/KeyAuth,EdgionPlugins/ProxyRewrite,EdgionPlugins/RateLimit,EdgionPlugins/RealIp,EdgionPlugins/ResponseRewrite,EdgionPlugins/RequestRestriction,EdgionPlugins/PathNormalization,EdgionPlugins/PathNormalization/ForwardView,EdgionPlugins/FaultInjection,EdgionPlugins/ForwardAuth,EdgionPlugins/OpenidConnect,EdgionPlugins/BandwidthLimit,EdgionPlugins/DirectEndpoint,EdgionPlugins/DynamicInternalUpstream,EdgionPlugins/DynamicExternalUpstream,EdgionPlugins/RegionRoute,EdgionPlugins/GlobalConnectionIpRestriction,EdgionPlugins/WebhookKeyGet,EdgionPlugins/Dsl,EdgionPlugins/DslGrpcError,EdgionPlugins/AllEndpointStatus"
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
        elif [ "$G_RESOURCE" = "LinkSys" ]; then
            # LinkSys/Redis assertions hit /api/v1/testing/link-sys/redis/* on the
            # Gateway admin port (covered by base) AND drive real proxy traffic
            # through edgion-plugins-gateway against the rate-limit-redis-* plugins
            # — load EdgionPlugins/base + RateLimitRedis so test 9 has its plugins.
            local linksys_extras="EdgionPlugins/base,EdgionPlugins/RateLimitRedis"
            if [ -z "$G_ITEM" ]; then
                suites="${base_suites},LinkSys/Status,LinkSys/Redis,${linksys_extras}"
            elif [ "$G_ITEM" = "Redis" ]; then
                suites="${base_suites},LinkSys/Redis,${linksys_extras}"
            else
                suites="${base_suites},LinkSys/${G_ITEM}"
            fi
        elif [ -n "$G_ITEM" ]; then
            if [ "$G_RESOURCE" = "Gateway" ] && [ "$G_ITEM" = "TLS/BackendTLS" ]; then
                suites="${base_suites},HTTPRoute/Backend/BackendTLS"
            elif [ "$G_RESOURCE" = "Gateway" ] && [ "$G_ITEM" = "TLS/BackendTLSSAN" ]; then
                suites="${base_suites},HTTPRoute/Backend/BackendTLSSAN,HTTPRoute/Backend/BackendTLS"
            elif [ "$G_RESOURCE" = "TCPRoute" ] && [ "$G_ITEM" = "StreamPlugins" ]; then
                # The ConnectionRateLimit Redis scenario (09_*) references the
                # default/redis-test LinkSys CRD, which only the LinkSys/Redis
                # suite defines — load it so targeted runs have the Redis client.
                suites="${base_suites},LinkSys/Redis,${G_RESOURCE}/${G_ITEM}"
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

    if is_redis_service_required "$suites"; then
        REDIS_SERVICE_REQUIRED=true
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

    # =============================================================================
    # Pre-flight Check 0: Only English letter scripts allowed outside docs/ and tasks/
    # =============================================================================
    log_section "Pre-flight Check: English-Only Lint"
    if ! "${PROJECT_ROOT}/cicd/checks/check_english_only.sh"; then
        log_error "English-only check failed — translate offending text to English before running integration tests"
        exit 1
    fi
    log_success "English-only check passed"

    # =============================================================================
    # Pre-flight Check 1: Agent-doc validation (links, frontmatter, indexes, ...)
    # =============================================================================
    log_section "Pre-flight Check: Agent-doc Validation"
    if ! python3 "${PROJECT_ROOT}/cicd/checks/validate_agent_docs.py"; then
        log_error "Agent-doc validation failed — fix doc consistency issues before running integration tests"
        exit 1
    fi
    log_success "Agent-doc validation passed"

    # =============================================================================
    # Pre-flight Check 2: edgion-resources leaf purity (no crate::core:: edges)
    # =============================================================================
    log_section "Pre-flight Check: Types Purity"
    if ! "${PROJECT_ROOT}/cicd/checks/check_types_purity.sh"; then
        log_error "Types-purity check failed — edgion-resources must stay a clean leaf (no crate::core:: edges)"
        exit 1
    fi
    log_success "Types-purity check passed"

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

    if ! start_redis_service_if_needed; then
        log_error "Failed to start external Redis dependency"
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

    # Step 1.6: Controller no-auth + token-auth startup (standalone, no Gateway).
    # Only runs in the default full sweep (no -r / -i targeting). Two phases:
    #   Phase 1 (no-auth): no [local_auth]/[auth] + no EDGION_ADMIN_TOKEN →
    #     /health and /api/v1/auth/status 200 (mode "token"); business route 503
    #     fail-close.
    #   Phase 2 (token auth): EDGION_ADMIN_TOKEN injected → business route 401
    #     without a Bearer header (auth active, not 503 fail-close) and 200 with the
    #     correct Bearer token.
    if [ -z "$G_RESOURCE" ]; then
        log_section "Controller no-auth and env-var-auth startup"
        if ! "${SCRIPT_DIR}/test_controller_no_auth.sh"; then
            log_error "FAIL: Controller no-auth/env-var-auth startup"
            exit 1
        fi
        log_success "Controller no-auth and env-var-auth startup passed"
    fi

    # Step 1.7: Gateway admin API auth (standalone preflight, boots its own
    # minimal Controller as the gRPC sync backend). Only runs in the default full
    # sweep (no -r / -i targeting). Verifies: no token configured ->
    # /configclient/* 503 fail-close (+ startup WARN); token configured -> 401
    # without a header and 200 with the correct Bearer token; the probe port
    # (/health, /ready) stays auth-free (200 without any token).
    if [ -z "$G_RESOURCE" ]; then
        log_section "Gateway admin auth"
        if ! "${SCRIPT_DIR}/test_gateway_admin_auth.sh"; then
            log_error "FAIL: Gateway admin auth"
            exit 1
        fi
        log_success "Gateway admin auth passed"
    fi

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
        local current_file="${PROJECT_ROOT}/tmp/integration_testing/.current"
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
        if [ -f "${PROJECT_ROOT}/tmp/integration_testing/.current" ]; then
            WORK_DIR=$(cat "${PROJECT_ROOT}/tmp/integration_testing/.current")
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
    local controller_url="http://127.0.0.1:12101"

    # =============================================================================
    # Test execution with optional reload
    # =============================================================================
    if $with_reload; then
        # ========== Round 1: Initial tests ==========
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
    if ! verify_metrics_labels "http://127.0.0.1:12190/metrics" "controller"; then
        log_error "Metrics label validation FAILED"
        test_failed=true
    fi

    # =============================================================================
    # Final Step: Requeue Dead Loop Detection
    # =============================================================================
    # Scan controller logs for trigger-chain cycle/depth errors that indicate a
    # requeue dead loop was detected and suppressed. These should never appear in
    # a healthy integration run; any occurrence is a bug in handler requeue logic.
    log_section "Final Step: Requeue Dead Loop Detection"
    local controller_log="${WORK_DIR}/logs/controller.log"
    if [ -f "$controller_log" ]; then
        local requeue_hits
        requeue_hits=$(grep -c \
            -e "Trigger cycle limit reached, dropping requeue" \
            -e "Trigger chain depth limit reached, dropping requeue" \
            "$controller_log" 2>/dev/null || true)
        if [ "${requeue_hits:-0}" -gt 0 ]; then
            log_error "Requeue dead loop detected: ${requeue_hits} occurrence(s) in controller.log"
            grep \
                -e "Trigger cycle limit reached, dropping requeue" \
                -e "Trigger chain depth limit reached, dropping requeue" \
                "$controller_log" | head -20 || true
            ((++FAILED_TESTS))
            test_failed=true
        else
            log_success "Requeue dead loop check passed (no dead loop events in controller log)"
        fi
    else
        log_warn "Requeue dead loop check skipped — controller.log not found at ${controller_log}"
    fi

    # =============================================================================
    # Final Step: Cache Residue Detection (leak detection across all scenarios)
    # =============================================================================
    if [ "$SKIP_RESIDUE_CHECK" != "true" ]; then
        log_section "Final Step: Cache Residue Detection"

        local residue_bin="${PROJECT_ROOT}/target/debug/cache_residue"
        # Always (re)build so the validator matches the current source. It changes
        # rarely, but a stale binary silently runs old logic or rejects new flags
        # (e.g. --controller-token), surfacing as a confusing rc=2 infra error.
        log_info "Building cache_residue (incremental)..."
        if ! (cd "$PROJECT_ROOT" && cargo build -p edgion-tests --bin cache_residue); then
            log_error "cache_residue build failed — skipping leak check"
            # do not fail the run on build failure; fall through
            residue_bin=""
        fi

        # The validator authenticates to the controller's now-standard-auth
        # diagnostic routes (wipe-all, store-stats), so obtain the admin token
        # first. ensure_ctrl_admin_token is idempotent and also covers the default
        # no-reload path, which otherwise never logs in. Without a token we cannot
        # authenticate, so skip the check rather than fail on a bogus 401.
        if [ -n "$residue_bin" ] && [ -x "$residue_bin" ]; then
            if ! ensure_ctrl_admin_token || [ -z "$CTRL_ADMIN_TOKEN" ]; then
                log_warn "Cache residue check skipped — controller admin token unavailable"
                residue_bin=""
            fi
        fi

        # Re-check: the token step above may have cleared residue_bin (skip case).
        if [ -n "$residue_bin" ] && [ -x "$residue_bin" ]; then
            # Always invoke validator with --mode fail so exit 1 unambiguously means
            # "residue detected". Shell-side RESIDUE_MODE then decides whether to
            # block the build (default: fail — opt out with RESIDUE_MODE=warn).
            # The controller's test-only diagnostic routes (wipe-all, store-stats)
            # require standard auth like any other admin route, so the validator
            # is given the admin Bearer token obtained at login. The gateway admin
            # API also requires a bearer token; the residue validator reads it from
            # the EDGION_GATEWAY_ADMIN_TOKEN env var (exported by start_all_with_conf.sh
            # and inherited here), so gateway-side residue checks run as well.
            local residue_mode="${RESIDUE_MODE:-fail}"
            # Disable errexit around the call so a non-zero exit (residue / infra
            # error) is captured into $rc and handled below instead of aborting.
            set +e
            "$residue_bin" \
                --controller-addr "http://127.0.0.1:12101" \
                --gateway-addr    "http://127.0.0.1:12001" \
                --controller-token "$CTRL_ADMIN_TOKEN" \
                --timeout-secs 30 \
                --mode fail
            local rc=$?
            set -e

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
    log_section "Test completed"
    log_success "All tests passed!"
}

main "$@"
