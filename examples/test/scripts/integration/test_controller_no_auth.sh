#!/usr/bin/env bash
# Integration test for the Controller starting up in two auth scenarios:
#
# PHASE 1 — no-auth (fail-close):
#   Start the Controller with no [local_auth] and no [auth] section.
#   Verify that, when nothing is configured:
#     1. Startup succeeds (no panic, no exit)
#     2. /health returns 200
#     3. /ready returns 200
#     4. /api/v1/auth/status returns 200
#     5. A business route (/api/v1/cluster/HTTPRoute) returns 503
#        (fail-close: require_auth=true + no provider ready -> 503)
#     6. Startup log contains the WARN "No authentication configured"
#
# PHASE 2 — env-var auth (positive path):
#   Start the Controller with EDGION_ADMIN_USERNAME / EDGION_ADMIN_PASSWORD /
#   EDGION_ADMIN_JWT_SECRET exported, but still NO [local_auth] section.
#   Verify that:
#     7. Login against /api/v1/auth/login with those credentials returns HTTP 200
#        and includes a token in the response body.
#   Rationale: HTTP 200 + token proves the env-var path synthesized a local-auth
#   provider.  The alternative assertion (business route 401 vs 503) is also
#   checked as a belt-and-suspenders guard.
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

CONTROLLER_BIN="${EDGION_CONTROLLER_BIN:-$REPO_ROOT/target/debug/edgion-controller}"

# Ports chosen to avoid conflicts with the main integration stack (5800/50051)
# and the Center no-auth test (58100/58110).
NOAUTH_ADMIN_PORT=58200
NOAUTH_GRPC_PORT=58201
ENVAUTH_ADMIN_PORT=58210
ENVAUTH_GRPC_PORT=58211

# =============================================================================
# Prerequisites
# =============================================================================
if [ ! -x "$CONTROLLER_BIN" ]; then
    echo "ERROR: edgion-controller binary not found at $CONTROLLER_BIN"
    echo "  Run: cargo build --bin edgion-controller"
    exit 1
fi

# =============================================================================
# Shared helpers
# =============================================================================

# Port precheck: abort early if the required port is already in use.
precheck_port() {
    local port="$1" label="$2"
    if lsof -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
        echo "FAIL: port $port ($label) already in use; stop the conflicting process or rerun later"
        lsof -iTCP:"$port" -sTCP:LISTEN 2>/dev/null | head -3
        return 1
    fi
}

# Poll a URL until it returns 200 or the timeout is reached.
# Also watches the process: if it exits early we abort.
wait_for_url_200() {
    local url="$1" pid="$2" label="$3"
    local i
    for i in $(seq 1 30); do
        local code
        code=$(curl -s -o /dev/null -w "%{http_code}" "$url" 2>/dev/null || true)
        if [ "$code" = "200" ]; then
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "FAIL: $label exited during startup"
            return 1
        fi
        sleep 1
    done
    echo "FAIL: $label $url did not return 200 within 30s"
    return 1
}

# Poll /health until the process is up or the timeout is reached.
wait_for_health() {
    local url="$1" pid="$2" label="$3"
    wait_for_url_200 "$url" "$pid" "$label"
}

# Assert that a URL returns the expected HTTP status code.
assert_status() {
    local step="$1" url="$2" expected="$3" logfile="$4"
    local actual
    actual=$(curl -s -o /dev/null -w "%{http_code}" "$url")
    if [ "$actual" != "$expected" ]; then
        echo "FAIL [$step]: $url returned $actual (expected $expected)"
        cat "$logfile"
        return 1
    fi
    echo "PASS [$step]: $url -> $actual"
}

# =============================================================================
# Phase 1: no-auth startup — expect fail-close 503 on business routes
#
# Body runs in a subshell `( ... )` so the EXIT trap fires exactly once when the
# subshell ends. A function-scoped `trap ... RETURN` would leak to the caller's
# frame and fire again on main()'s return, where the locals are out of scope
# (CTRL_PID unbound under `set -u`).
# =============================================================================
run_phase1() (
    local TMPDIR
    TMPDIR=$(mktemp -d)

    # Work dir layout: controller expects config/crd under work_dir
    local CONFIG_DIR="${TMPDIR}/config"
    local LOG_DIR="${TMPDIR}/logs"
    local CONF_DIR="${TMPDIR}/conf"
    mkdir -p "$CONFIG_DIR" "$LOG_DIR" "$CONF_DIR"

    # Copy CRD schemas — the Controller exits with an error in file_system mode
    # if config/crd is absent or empty.
    if [ -d "${REPO_ROOT}/config/crd" ]; then
        cp -R "${REPO_ROOT}/config/crd/." "${CONFIG_DIR}/crd/"
    else
        echo "FAIL: CRD schema source not found: ${REPO_ROOT}/config/crd"
        rm -rf "$TMPDIR"
        return 1
    fi

    # Minimal controller config — NO [local_auth], NO [auth] section.
    cat > "$TMPDIR/controller.yaml" <<EOF
conf_center:
  type: file_system
  conf_dir: ${CONF_DIR}
EOF

    local CTRL_PID=""
    cleanup_phase1() {
        [ -n "$CTRL_PID" ] && kill "$CTRL_PID" 2>/dev/null || true
        [ -n "$CTRL_PID" ] && wait "$CTRL_PID" 2>/dev/null || true
        rm -rf "$TMPDIR"
    }
    trap cleanup_phase1 EXIT

    # Ensure EDGION_ADMIN_* env vars are not set so no auth provider synthesizes.
    unset EDGION_ADMIN_USERNAME EDGION_ADMIN_PASSWORD EDGION_ADMIN_JWT_SECRET

    # Port prechecks
    precheck_port "$NOAUTH_ADMIN_PORT" "controller-noauth admin" || return 1
    precheck_port "$NOAUTH_GRPC_PORT"  "controller-noauth gRPC"  || return 1

    echo ""
    echo "=== Phase 1: Controller no-auth startup ==="

    "$CONTROLLER_BIN" \
        -c "$TMPDIR/controller.yaml" \
        --work-dir "$TMPDIR" \
        --log-dir "$LOG_DIR" \
        --admin-listen "127.0.0.1:${NOAUTH_ADMIN_PORT}" \
        --grpc-listen "127.0.0.1:${NOAUTH_GRPC_PORT}" \
        > "$LOG_DIR/controller.log" 2>&1 &
    CTRL_PID=$!

    if ! wait_for_health \
            "http://127.0.0.1:${NOAUTH_ADMIN_PORT}/health" \
            "$CTRL_PID" \
            "controller (no-auth)"; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi

    assert_status "1/6" \
        "http://127.0.0.1:${NOAUTH_ADMIN_PORT}/health" \
        "200" "$LOG_DIR/controller.log" || return 1

    # /ready reflects ConfMgr initialization state (not auth); poll until 200.
    echo "[2/6] Waiting for /ready..."
    if ! wait_for_url_200 \
            "http://127.0.0.1:${NOAUTH_ADMIN_PORT}/ready" \
            "$CTRL_PID" \
            "controller /ready (no-auth)"; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    echo "PASS [2/6]: http://127.0.0.1:${NOAUTH_ADMIN_PORT}/ready -> 200"

    assert_status "3/6" \
        "http://127.0.0.1:${NOAUTH_ADMIN_PORT}/api/v1/auth/status" \
        "200" "$LOG_DIR/controller.log" || return 1

    # Business route: list cluster-scoped HTTPRoute resources.
    # Without a provider loaded the middleware returns 503 fail-close (not 401).
    assert_status "4/6" \
        "http://127.0.0.1:${NOAUTH_ADMIN_PORT}/api/v1/cluster/HTTPRoute" \
        "503" "$LOG_DIR/controller.log" || return 1

    echo "[5/6] Checking startup WARN log..."
    if ! grep -q "No authentication configured" "$LOG_DIR/controller.log"; then
        echo "FAIL [5/6]: Expected WARN 'No authentication configured' not found in log"
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    echo "PASS [5/6]: WARN 'No authentication configured' found in log"

    echo "PASS: Phase 1 (Controller no-auth startup) complete"
)

# =============================================================================
# Phase 2: env-var auth — expect login to succeed and business route to be 401
#
# Subshell body + EXIT trap, same rationale as run_phase1. Running in a subshell
# also contains the EDGION_ADMIN_* exports below so they cannot leak to main.
# =============================================================================
run_phase2() (
    local TMPDIR
    TMPDIR=$(mktemp -d)

    local CONFIG_DIR="${TMPDIR}/config"
    local LOG_DIR="${TMPDIR}/logs"
    local CONF_DIR="${TMPDIR}/conf"
    mkdir -p "$CONFIG_DIR" "$LOG_DIR" "$CONF_DIR"

    if [ -d "${REPO_ROOT}/config/crd" ]; then
        cp -R "${REPO_ROOT}/config/crd/." "${CONFIG_DIR}/crd/"
    else
        echo "FAIL: CRD schema source not found: ${REPO_ROOT}/config/crd"
        rm -rf "$TMPDIR"
        return 1
    fi

    # Minimal controller config — still NO [local_auth] section.
    # Credentials come entirely from the env vars set below.
    cat > "$TMPDIR/controller.yaml" <<EOF
conf_center:
  type: file_system
  conf_dir: ${CONF_DIR}
EOF

    local CTRL_PID=""
    cleanup_phase2() {
        [ -n "$CTRL_PID" ] && kill "$CTRL_PID" 2>/dev/null || true
        [ -n "$CTRL_PID" ] && wait "$CTRL_PID" 2>/dev/null || true
        rm -rf "$TMPDIR"
        # Restore env to a clean state for any caller.
        unset EDGION_ADMIN_USERNAME EDGION_ADMIN_PASSWORD EDGION_ADMIN_JWT_SECRET
    }
    trap cleanup_phase2 EXIT

    # Env-var credentials — obviously fake test values, never real secrets.
    export EDGION_ADMIN_USERNAME="admin"
    export EDGION_ADMIN_PASSWORD="test-admin-pw-123"
    # JWT secret must be >= 32 bytes; use a 40-char dummy.
    export EDGION_ADMIN_JWT_SECRET="test-jwt-secret-edgion-no-auth-integ-test-xyz"

    precheck_port "$ENVAUTH_ADMIN_PORT" "controller-envauth admin" || return 1
    precheck_port "$ENVAUTH_GRPC_PORT"  "controller-envauth gRPC"  || return 1

    echo ""
    echo "=== Phase 2: Controller env-var auth startup ==="

    "$CONTROLLER_BIN" \
        -c "$TMPDIR/controller.yaml" \
        --work-dir "$TMPDIR" \
        --log-dir "$LOG_DIR" \
        --admin-listen "127.0.0.1:${ENVAUTH_ADMIN_PORT}" \
        --grpc-listen "127.0.0.1:${ENVAUTH_GRPC_PORT}" \
        > "$LOG_DIR/controller.log" 2>&1 &
    CTRL_PID=$!

    if ! wait_for_health \
            "http://127.0.0.1:${ENVAUTH_ADMIN_PORT}/health" \
            "$CTRL_PID" \
            "controller (env-auth)"; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi

    # Primary assertion: login with the env-injected credentials must return HTTP 200
    # and include a token in the body.  This proves the env-var path synthesized a
    # valid local-auth provider (no [local_auth] section was present in the config).
    echo "[6/6] Testing login with env-var credentials..."
    local LOGIN_RESP
    LOGIN_RESP=$(curl -s -o /dev/null -w "%{http_code}" \
        -H "Content-Type: application/json" \
        -d "{\"username\":\"admin\",\"password\":\"test-admin-pw-123\"}" \
        "http://127.0.0.1:${ENVAUTH_ADMIN_PORT}/api/v1/auth/login")

    if [ "$LOGIN_RESP" != "200" ]; then
        echo "FAIL [6/6]: /api/v1/auth/login returned $LOGIN_RESP (expected 200)"
        cat "$LOG_DIR/controller.log"
        return 1
    fi

    # Belt-and-suspenders: verify token is present in the response body.
    local LOGIN_BODY
    LOGIN_BODY=$(curl -sf \
        -H "Content-Type: application/json" \
        -d "{\"username\":\"admin\",\"password\":\"test-admin-pw-123\"}" \
        "http://127.0.0.1:${ENVAUTH_ADMIN_PORT}/api/v1/auth/login" 2>/dev/null || true)
    if ! echo "$LOGIN_BODY" | grep -q '"token"'; then
        echo "FAIL [6/6]: Login response does not contain a token field"
        echo "Response: $LOGIN_BODY"
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    echo "PASS [6/6]: Login with env-var credentials returned 200 and includes a token"

    # Secondary assertion: a business route without a token returns 401 (auth active),
    # not 503 (no provider).  This distinguishes the env-auth case from the no-auth case.
    assert_status "belt-and-suspenders" \
        "http://127.0.0.1:${ENVAUTH_ADMIN_PORT}/api/v1/cluster/HTTPRoute" \
        "401" "$LOG_DIR/controller.log" || return 1

    echo "PASS: Phase 2 (Controller env-var auth) complete"
)

# =============================================================================
# Entry point
# =============================================================================
main() {
    run_phase1
    run_phase2
    echo ""
    echo "PASS: Controller no-auth and env-var-auth startup scenarios"
}

main
