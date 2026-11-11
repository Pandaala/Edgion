#!/usr/bin/env bash
# =============================================================================
# edgion-cli End-to-End Integration Test
#
# Verifies 5 subcommands work end-to-end against a live controller:
#   1. login     — persist a pre-shared token, empty-token rejection (token-only)
#   2. apply     — single file, multi-doc, nonexistent file, invalid YAML
#   3. get       — list all, single resource, with -o yaml, unknown kind
#   4. delete    — existing resource, nonexistent resource, delete-then-verify
#   5. gen-certs — file output mode (no K8s required), idempotent with --force
#
# Actual subcommand flags used (verified against source):
#   login:     --server <URL> --token <TOKEN>
#   apply:     --file/-f <path> [--dry-run]
#   get:       <kind> [<name>] [-n <ns>] [-o table|json|yaml|wide]
#   delete:    <kind> <name> [-n <ns>]  OR  --file <path>
#   gen-certs: --output file --output-dir <dir> [--force]
#
# Usage:
#   ./test_cli_commands.sh                   # full run: start, test, stop
#   ./test_cli_commands.sh --no-start        # assume controller already running
#   ./test_cli_commands.sh --keep-alive      # don't stop services after test
#   ./test_cli_commands.sh --no-start --keep-alive
#
# Environment:
#   EDGION_WORK_DIR   — overrides the work directory detection (used by auto_auth)
#   CONTROLLER_URL    — default: http://127.0.0.1:12101
# =============================================================================
set -euo pipefail

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
UTILS_DIR="${SCRIPT_DIR}/../utils"

# Binary selection (mirrors coverage mode logic used in other scripts)
if [ "${EDGION_COVERAGE:-0}" = "1" ]; then
    eval "$(cargo llvm-cov show-env --export-prefix 2>/dev/null)"
    export CARGO_TARGET_DIR="${PROJECT_ROOT}/target/llvm-cov-target"
    export LLVM_PROFILE_FILE="${PROJECT_ROOT}/target/llvm-cov-target/Edgion-%p-%10m.profraw"
    BIN_DIR="${PROJECT_ROOT}/target/llvm-cov-target/debug"
else
    BIN_DIR="${PROJECT_ROOT}/target/debug"
fi

CLI="${BIN_DIR}/edgion-cli"
CONTROLLER_URL="${CONTROLLER_URL:-http://127.0.0.1:12101}"
CONTROLLER_PROBE_URL="${CONTROLLER_PROBE_URL:-http://127.0.0.1:12100}"
CONTROLLER_ADMIN_PORT=12101

# ---------------------------------------------------------------------------
# Colours
# ---------------------------------------------------------------------------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# ---------------------------------------------------------------------------
# Parse flags
# ---------------------------------------------------------------------------
NO_START=false
KEEP_ALIVE=false

for arg in "$@"; do
    case "$arg" in
        --no-start)   NO_START=true ;;
        --keep-alive) KEEP_ALIVE=true ;;
        -h|--help)
            grep '^#' "$0" | head -30 | sed 's/^# \?//'
            exit 0
            ;;
        *)
            echo "Unknown option: $arg" >&2
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Test counters
# ---------------------------------------------------------------------------
PASSED=0
FAILED=0

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_ok()      { echo -e "${GREEN}[PASS]${NC} $1"; }
log_fail()    { echo -e "${RED}[FAIL]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${BLUE}============================================================${NC}"
    echo -e "${BLUE} $1${NC}"
    echo -e "${BLUE}============================================================${NC}"
}

# ---------------------------------------------------------------------------
# Assertion helpers
# ---------------------------------------------------------------------------

# pass_case <desc>
pass_case() {
    PASSED=$((PASSED + 1))
    log_ok "$1"
}

# fail_case <desc> [extra info]
fail_case() {
    FAILED=$((FAILED + 1))
    log_fail "$1"
    if [ -n "${2:-}" ]; then
        echo "         $2"
    fi
}

# assert_exit0 <desc> <cmd...>
# Runs the command; passes if it exits 0.
assert_exit0() {
    local desc="$1"; shift
    local actual_out
    local actual_rc=0
    actual_out=$("$@" 2>&1) || actual_rc=$?
    if [ "$actual_rc" -eq 0 ]; then
        pass_case "$desc"
    else
        fail_case "$desc" "expected exit 0, got $actual_rc — output: $(echo "$actual_out" | head -5)"
    fi
}

# assert_exit_nonzero <desc> <cmd...>
# Runs the command; passes if it exits non-zero.
assert_exit_nonzero() {
    local desc="$1"; shift
    local actual_out
    local actual_rc=0
    actual_out=$("$@" 2>&1) || actual_rc=$?
    if [ "$actual_rc" -ne 0 ]; then
        pass_case "$desc"
    else
        fail_case "$desc" "expected non-zero exit, got 0 — output: $(echo "$actual_out" | head -5)"
    fi
}

# assert_exit0_contains <desc> <pattern> <cmd...>
# Runs the command; passes if exit 0 and stdout/stderr contains pattern.
assert_exit0_contains() {
    local desc="$1"
    local pattern="$2"
    shift 2
    local actual_out
    local actual_rc=0
    actual_out=$("$@" 2>&1) || actual_rc=$?
    if [ "$actual_rc" -eq 0 ] && echo "$actual_out" | grep -qiF "$pattern"; then
        pass_case "$desc"
    elif [ "$actual_rc" -ne 0 ]; then
        fail_case "$desc" "expected exit 0, got $actual_rc — output: $(echo "$actual_out" | head -5)"
    else
        fail_case "$desc" "expected output to contain '$pattern' — output: $(echo "$actual_out" | head -5)"
    fi
}

# assert_exit_nonzero_contains <desc> <pattern> <cmd...>
# Runs the command; passes if exit non-zero and combined output contains pattern.
assert_exit_nonzero_contains() {
    local desc="$1"
    local pattern="$2"
    shift 2
    local actual_out
    local actual_rc=0
    actual_out=$("$@" 2>&1) || actual_rc=$?
    if [ "$actual_rc" -ne 0 ] && echo "$actual_out" | grep -qiF "$pattern"; then
        pass_case "$desc"
    elif [ "$actual_rc" -eq 0 ]; then
        fail_case "$desc" "expected non-zero exit, got 0 — output: $(echo "$actual_out" | head -5)"
    else
        fail_case "$desc" "expected output to contain '$pattern' — got: $(echo "$actual_out" | head -5)"
    fi
}

# ---------------------------------------------------------------------------
# Temp directory for test fixtures
# ---------------------------------------------------------------------------
TMP_DIR="$(mktemp -d)"
cleanup_tmp() { rm -rf "$TMP_DIR"; }
trap cleanup_tmp EXIT

# ---------------------------------------------------------------------------
# Service lifecycle
# ---------------------------------------------------------------------------
SERVICES_STARTED=false

start_services() {
    log_section "Starting services (HTTPRoute/Basic)"
    # Clear stale token so auto_auth picks up fresh credentials
    rm -f ~/.edgion/token

    bash "${UTILS_DIR}/start_all_with_conf.sh" --suites "HTTPRoute/Basic"

    # After start_all_with_conf.sh exits, read the work dir from .current
    if [ -f "${PROJECT_ROOT}/tmp/integration_testing/.current" ]; then
        export EDGION_WORK_DIR="$(cat "${PROJECT_ROOT}/tmp/integration_testing/.current")"
        log_info "EDGION_WORK_DIR=$EDGION_WORK_DIR"
    fi

    SERVICES_STARTED=true
    log_info "Services started successfully"
}

stop_services() {
    if $SERVICES_STARTED; then
        log_section "Stopping services"
        bash "${UTILS_DIR}/kill_all.sh" || true
    fi
}

# ---------------------------------------------------------------------------
# Wait for controller health
# ---------------------------------------------------------------------------
wait_for_controller() {
    local timeout=30
    local elapsed=0
    log_info "Waiting for controller at $CONTROLLER_PROBE_URL..."
    while [ "$elapsed" -lt "$timeout" ]; do
        if curl -sf "${CONTROLLER_PROBE_URL}/health" > /dev/null 2>&1; then
            log_info "Controller ready"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    echo "ERROR: controller did not become ready within ${timeout}s" >&2
    exit 1
}

# ---------------------------------------------------------------------------
# Extract admin credentials from controller Secret
# ---------------------------------------------------------------------------
# Token-only admin auth. The Controller reads a single pre-shared bearer token
# from EDGION_ADMIN_TOKEN; export it (honoring an inherited value) at load time —
# i.e. BEFORE main() calls start_services — so the Controller under test and the
# CLI agree on the token. edgion-cli reads the same value via EDGION_TOKEN.
export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
export EDGION_TOKEN="${EDGION_ADMIN_TOKEN}"
ADMIN_TOKEN="${EDGION_ADMIN_TOKEN}"

detect_admin_credentials() {
    # The admin token comes from the EDGION_ADMIN_TOKEN env var exported above.
    ADMIN_TOKEN="${EDGION_ADMIN_TOKEN}"
    if [ -z "$ADMIN_TOKEN" ]; then
        echo "ERROR: EDGION_ADMIN_TOKEN not set — cannot determine admin token" >&2
        exit 1
    fi
    log_info "Using admin token from EDGION_ADMIN_TOKEN"
}

# ---------------------------------------------------------------------------
# CLI shorthand (always explicit server; no interactive prompts)
# ---------------------------------------------------------------------------
cli() {
    "$CLI" --server "$CONTROLLER_URL" "$@"
}

cli_authed() {
    # Uses saved token (from login) or auto_auth via EDGION_WORK_DIR
    "$CLI" --server "$CONTROLLER_URL" "$@"
}

# ---------------------------------------------------------------------------
# Test fixtures written to TMP_DIR
# ---------------------------------------------------------------------------
write_fixtures() {
    # Single-doc HTTPRoute (test namespace)
    cat > "${TMP_DIR}/single-httproute.yaml" <<'YAML'
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: cli-test-route
  namespace: edgion-test
spec:
  parentRefs:
    - name: http-test-gateway
      namespace: edgion-test
  hostnames:
    - "cli-test.example.com"
  rules:
    - matches:
        - path:
            type: Exact
            value: /ping
      backendRefs:
        - name: test-http
          port: 30001
YAML

    # Multi-doc YAML (two resources in one file)
    cat > "${TMP_DIR}/multi-doc.yaml" <<'YAML'
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: cli-multi-a
  namespace: edgion-test
spec:
  parentRefs:
    - name: http-test-gateway
      namespace: edgion-test
  rules:
    - backendRefs:
        - name: test-http
          port: 30001
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: cli-multi-b
  namespace: edgion-test
spec:
  parentRefs:
    - name: http-test-gateway
      namespace: edgion-test
  rules:
    - backendRefs:
        - name: test-http
          port: 30001
YAML

    # Invalid YAML (parse error)
    cat > "${TMP_DIR}/invalid.yaml" <<'YAML'
this: is: not: valid: yaml: ][{
YAML
}

# ===========================================================================
# ===  TEST SUITE: login  ===================================================
# ===========================================================================
run_login_tests() {
    log_section "1. login (token-only)"

    # Case 1.1: login persists a non-empty pre-shared token → exit 0 + confirmation.
    #   `login` now only persists the token to ~/.edgion/token (the reachability
    #   probe is best-effort and never fatal).
    assert_exit0_contains \
        "login: persists a pre-shared token" \
        "Token saved" \
        "$CLI" --server "$CONTROLLER_URL" login --token "$ADMIN_TOKEN"

    # Case 1.2: empty token rejected at the CLI layer (before any HTTP) → exit non-zero
    assert_exit_nonzero \
        "login: empty token → exit non-zero" \
        "$CLI" --server "$CONTROLLER_URL" login --token ""
}

# ===========================================================================
# ===  TEST SUITE: apply  ===================================================
# ===========================================================================
run_apply_tests() {
    log_section "2. apply"

    # Ensure the pre-shared token is persisted before apply tests
    "$CLI" --server "$CONTROLLER_URL" login --token "$ADMIN_TOKEN" > /dev/null 2>&1

    # Case 2.1: apply single valid file → exit 0, output contains resource name
    assert_exit0_contains \
        "apply: single valid YAML file" \
        "cli-test-route" \
        cli_authed apply --file "${TMP_DIR}/single-httproute.yaml"

    # Case 2.2: apply multi-doc YAML → exit 0, both resources mentioned
    local multi_out
    local multi_rc=0
    multi_out=$(cli_authed apply --file "${TMP_DIR}/multi-doc.yaml" 2>&1) || multi_rc=$?
    if [ "$multi_rc" -eq 0 ] && echo "$multi_out" | grep -q "cli-multi-a" && echo "$multi_out" | grep -q "cli-multi-b"; then
        pass_case "apply: multi-doc YAML (both docs applied)"
    elif [ "$multi_rc" -ne 0 ]; then
        fail_case "apply: multi-doc YAML" "exit $multi_rc — $multi_out"
    else
        fail_case "apply: multi-doc YAML" "expected both 'cli-multi-a' and 'cli-multi-b' in output — got: $(echo "$multi_out" | head -5)"
    fi

    # Case 2.3: apply nonexistent file → exit non-zero, "not found" in message
    assert_exit_nonzero_contains \
        "apply: nonexistent file → exit non-zero" \
        "not found" \
        cli_authed apply --file "${TMP_DIR}/does-not-exist.yaml"

    # Case 2.4: apply invalid YAML → exit non-zero, error message
    assert_exit_nonzero \
        "apply: invalid YAML → exit non-zero" \
        cli_authed apply --file "${TMP_DIR}/invalid.yaml"

    # Case 2.5: apply with --dry-run → exit 0, no server-side change (shows "Would apply")
    assert_exit0_contains \
        "apply: --dry-run prints intent without applying" \
        "Would apply" \
        cli_authed apply --file "${TMP_DIR}/single-httproute.yaml" --dry-run
}

# ===========================================================================
# ===  TEST SUITE: get  =====================================================
# ===========================================================================
run_get_tests() {
    log_section "3. get"

    # Case 3.1: get httproute → exit 0, list output (previously applied routes visible)
    assert_exit0 \
        "get: list HTTPRoute exits 0" \
        cli_authed get httproute

    # Case 3.2: output contains the route we applied
    local get_out
    local get_rc=0
    get_out=$(cli_authed get httproute 2>&1) || get_rc=$?
    if [ "$get_rc" -eq 0 ] && echo "$get_out" | grep -q "cli-test-route"; then
        pass_case "get: applied cli-test-route appears in list"
    elif [ "$get_rc" -ne 0 ]; then
        fail_case "get: list HTTPRoute" "exit $get_rc — $get_out"
    else
        fail_case "get: cli-test-route visible in listing" "route not found in output: $(echo "$get_out" | head -10)"
    fi

    # Case 3.3: get single resource by name → exit 0
    assert_exit0 \
        "get: single resource by name" \
        cli_authed get httproute cli-test-route -n edgion-test

    # Case 3.4: get with -o yaml output → exit 0, contains "apiVersion"
    assert_exit0_contains \
        "get: -o yaml output contains apiVersion" \
        "apiVersion" \
        cli_authed get httproute cli-test-route -n edgion-test -o yaml

    # Case 3.5: get with -o json output → exit 0, contains a JSON brace
    assert_exit0_contains \
        "get: -o json output is JSON-like" \
        "{" \
        cli_authed get httproute -o json

    # Case 3.6: get nonexistent resource name → exit non-zero (404)
    assert_exit_nonzero \
        "get: nonexistent resource name → exit non-zero" \
        cli_authed get httproute no-such-route-xyz -n edgion-test

    # Case 3.7: get with namespace filter → exit 0
    assert_exit0 \
        "get: list with -n namespace filter exits 0" \
        cli_authed get httproute -n edgion-test
}

# ===========================================================================
# ===  TEST SUITE: delete  ==================================================
# ===========================================================================
run_delete_tests() {
    log_section "4. delete"

    # Case 4.1: delete existing resource → exit 0, output contains "deleted"
    assert_exit0_contains \
        "delete: existing resource" \
        "deleted" \
        cli_authed delete httproute cli-multi-a -n edgion-test

    # Case 4.2: delete second resource from multi-doc apply
    assert_exit0_contains \
        "delete: second multi-doc resource" \
        "deleted" \
        cli_authed delete httproute cli-multi-b -n edgion-test

    # Case 4.3: after delete, get by name → exit non-zero (404)
    assert_exit_nonzero \
        "delete: deleted resource no longer found via get" \
        cli_authed get httproute cli-multi-a -n edgion-test

    # Case 4.4: delete nonexistent resource → exit non-zero
    assert_exit_nonzero \
        "delete: nonexistent resource → exit non-zero" \
        cli_authed delete httproute no-such-route-xyz -n edgion-test

    # Case 4.5: delete via --file flag using the single-doc fixture
    #   First re-apply so we have something to delete
    cli_authed apply --file "${TMP_DIR}/single-httproute.yaml" > /dev/null 2>&1 || true
    assert_exit0_contains \
        "delete: delete via --file flag" \
        "deleted" \
        cli_authed delete --file "${TMP_DIR}/single-httproute.yaml"
}

# ===========================================================================
# ===  TEST SUITE: gen-certs  ===============================================
# ===========================================================================
run_gen_certs_tests() {
    log_section "5. gen-certs"

    # Case 5.1: gen-certs --output file --output-dir → exit 0
    assert_exit0 \
        "gen-certs: generates certs in file mode" \
        "$CLI" gen-certs --output file --output-dir "${TMP_DIR}/test-certs-output"

    # Case 5.2: output directory contains expected PEM files
    local expected_files=( "ca.crt" "server.crt" "server.key" "client.crt" "client.key" )
    local cert_out_dir="${TMP_DIR}/test-certs-output/primary"
    local all_present=true
    for f in "${expected_files[@]}"; do
        if [ ! -f "${cert_out_dir}/${f}" ]; then
            all_present=false
            fail_case "gen-certs: ${f} present in output directory"
        fi
    done
    if $all_present; then
        pass_case "gen-certs: all 5 cert/key files present (ca.crt, server.crt, server.key, client.crt, client.key)"
    fi

    # Case 5.3: ca.crt is a valid PEM certificate
    if [ -f "${cert_out_dir}/ca.crt" ] && grep -q "BEGIN CERTIFICATE" "${cert_out_dir}/ca.crt"; then
        pass_case "gen-certs: ca.crt is valid PEM format"
    else
        fail_case "gen-certs: ca.crt is valid PEM format" "file missing or does not start with PEM header"
    fi

    # Case 5.4: server.key is a valid PEM private key
    if [ -f "${cert_out_dir}/server.key" ] && grep -q "BEGIN" "${cert_out_dir}/server.key"; then
        pass_case "gen-certs: server.key is valid PEM format"
    else
        fail_case "gen-certs: server.key is valid PEM format" "file missing or does not start with PEM header"
    fi

    # Case 5.5: second run without --force → exit non-zero (files exist)
    assert_exit_nonzero_contains \
        "gen-certs: second run without --force → refuses to overwrite" \
        "already exist" \
        "$CLI" gen-certs --output file --output-dir "${TMP_DIR}/test-certs-output"

    # Case 5.6: second run with --force → exit 0 (overwrites)
    assert_exit0 \
        "gen-certs: --force overwrites existing certs" \
        "$CLI" gen-certs --output file --output-dir "${TMP_DIR}/test-certs-output" --force

    # Case 5.7: custom --cert-name creates a subdirectory with that name
    assert_exit0 \
        "gen-certs: --cert-name creates named subdirectory" \
        "$CLI" gen-certs --output file --output-dir "${TMP_DIR}/named-certs" --cert-name rotation-test
    if [ -f "${TMP_DIR}/named-certs/rotation-test/ca.crt" ]; then
        pass_case "gen-certs: --cert-name subdirectory exists"
    else
        fail_case "gen-certs: --cert-name subdirectory exists" "expected ${TMP_DIR}/named-certs/rotation-test/ca.crt"
    fi
}

# ===========================================================================
# ===  MAIN  ================================================================
# ===========================================================================
main() {
    echo ""
    echo -e "${BLUE}========================================================${NC}"
    echo -e "${BLUE} edgion-cli End-to-End Integration Test${NC}"
    echo -e "${BLUE}========================================================${NC}"
    echo "Project root : ${PROJECT_ROOT}"
    echo "CLI binary   : ${CLI}"
    echo "Controller   : ${CONTROLLER_URL}"
    echo "Temp dir     : ${TMP_DIR}"
    echo ""

    # Verify CLI binary exists
    if [ ! -x "$CLI" ]; then
        echo "ERROR: edgion-cli binary not found at $CLI" >&2
        echo "  Run: cargo build --bin edgion-cli" >&2
        exit 1
    fi

    # Start services if requested
    if ! $NO_START; then
        start_services
        if ! $KEEP_ALIVE; then
            trap 'stop_services' EXIT
        fi
    fi

    # Wait for controller to be healthy
    wait_for_controller

    # Detect admin credentials from the Secret file written by controller
    detect_admin_credentials

    # Write test fixture files
    write_fixtures

    # --- Run test suites ---
    run_login_tests
    run_apply_tests
    run_get_tests
    run_delete_tests
    run_gen_certs_tests

    # --- Summary ---
    echo ""
    echo -e "${BLUE}========================================================${NC}"
    echo -e "${BLUE} Test Summary${NC}"
    echo -e "${BLUE}========================================================${NC}"
    echo -e "  ${GREEN}PASSED${NC}: ${PASSED}"
    if [ "$FAILED" -gt 0 ]; then
        echo -e "  ${RED}FAILED${NC}: ${FAILED}"
    else
        echo -e "  FAILED: ${FAILED}"
    fi
    echo ""

    if [ "$FAILED" -gt 0 ]; then
        echo -e "${RED}RESULT: FAILED ($FAILED failure(s))${NC}"
        exit 1
    else
        echo -e "${GREEN}RESULT: PASSED (all $PASSED tests passed)${NC}"
        exit 0
    fi
}

main
