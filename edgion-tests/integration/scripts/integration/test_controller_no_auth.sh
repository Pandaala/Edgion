#!/usr/bin/env bash
# Integration test for the Controller starting up in two auth scenarios
# (token-only model: a single pre-shared bearer token read from EDGION_ADMIN_TOKEN):
#
# PHASE 1 — no-auth (fail-close):
#   Start the Controller with no [local_auth]/[auth] section and no
#   EDGION_ADMIN_TOKEN. Verify that, when nothing is configured:
#     1. Startup succeeds (no panic, no exit)
#     2. /health returns 200
#     3. /ready returns 200
#     4. /api/v1/auth/status returns 200 and reports mode "token"
#     5. A business route (/api/v1/namespaced/HTTPRoute) returns 503
#        (fail-close: require_auth=true + no token registered -> 503)
#
# PHASE 2 — token auth (positive path):
#   Start the Controller with EDGION_ADMIN_TOKEN exported (still NO [local_auth]
#   section). Verify that:
#     6a. A business route WITHOUT a Bearer header returns 401 (auth active, not
#         503 fail-close) — proves the env-var token synthesized an active provider.
#     6b. The same route WITH the correct Bearer token returns 200.
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

UTILS_DIR="${REPO_ROOT}/edgion-tests/integration/scripts/utils"
CONTROLLER_BIN="${EDGION_CONTROLLER_BIN:-$REPO_ROOT/target/debug/edgion-controller}"
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

# Ports chosen to avoid conflicts with the main integration stack (12101/12151)
# and the Center no-auth test (58100/58110).
#
# /health and /ready are served on the dedicated probe listener (not the admin
# API), and /metrics on the dedicated metrics listener. We pin both to
# test-specific ports via --probe-listen / --metrics-listen so they do not fall
# back to the shared defaults (0.0.0.0:12100 / 0.0.0.0:12190), which would
# collide with the main integration controller.
NOAUTH_ADMIN_PORT=58200
NOAUTH_GRPC_PORT=58201
NOAUTH_PROBE_PORT=58202
NOAUTH_METRICS_PORT=58203
ENVAUTH_ADMIN_PORT=58210
ENVAUTH_GRPC_PORT=58211
ENVAUTH_PROBE_PORT=58212
ENVAUTH_METRICS_PORT=58213

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
    # conf_sync is mTLS by default: certs land under the work dir (TMPDIR) at the
    # work-dir relative paths the shared emitter writes into the config below.
    conf_sync_distribute_certs "$TMPDIR"

    cat > "$TMPDIR/controller.yaml" <<EOF
conf_center:
  type: file_system
  conf_dir: ${CONF_DIR}
EOF
    conf_sync_security_block server >> "$TMPDIR/controller.yaml"

    local CTRL_PID=""
    cleanup_phase1() {
        [ -n "$CTRL_PID" ] && kill "$CTRL_PID" 2>/dev/null || true
        [ -n "$CTRL_PID" ] && wait "$CTRL_PID" 2>/dev/null || true
        rm -rf "$TMPDIR"
    }
    trap cleanup_phase1 EXIT

    # Ensure EDGION_ADMIN_TOKEN is not set so no admin token is registered.
    unset EDGION_ADMIN_TOKEN

    # Port prechecks
    precheck_port "$NOAUTH_ADMIN_PORT" "controller-noauth admin" || return 1
    precheck_port "$NOAUTH_GRPC_PORT"  "controller-noauth gRPC"  || return 1
    precheck_port "$NOAUTH_PROBE_PORT" "controller-noauth probe" || return 1

    echo ""
    echo "=== Phase 1: Controller no-auth startup ==="

    "$CONTROLLER_BIN" \
        -c "$TMPDIR/controller.yaml" \
        --work-dir "$TMPDIR" \
        --log-dir "$LOG_DIR" \
        --admin-listen "127.0.0.1:${NOAUTH_ADMIN_PORT}" \
        --grpc-listen "127.0.0.1:${NOAUTH_GRPC_PORT}" \
        --probe-listen "127.0.0.1:${NOAUTH_PROBE_PORT}" \
        --metrics-listen "127.0.0.1:${NOAUTH_METRICS_PORT}" \
        > "$LOG_DIR/controller.log" 2>&1 &
    CTRL_PID=$!

    # /health and /ready live on the dedicated probe listener (probe port).
    if ! wait_for_health \
            "http://127.0.0.1:${NOAUTH_PROBE_PORT}/health" \
            "$CTRL_PID" \
            "controller (no-auth)"; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi

    assert_status "1/6" \
        "http://127.0.0.1:${NOAUTH_PROBE_PORT}/health" \
        "200" "$LOG_DIR/controller.log" || return 1

    # /ready reflects ConfMgr initialization state (not auth); poll until 200.
    echo "[2/6] Waiting for /ready..."
    if ! wait_for_url_200 \
            "http://127.0.0.1:${NOAUTH_PROBE_PORT}/ready" \
            "$CTRL_PID" \
            "controller /ready (no-auth)"; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    echo "PASS [2/6]: http://127.0.0.1:${NOAUTH_PROBE_PORT}/ready -> 200"

    assert_status "3/5" \
        "http://127.0.0.1:${NOAUTH_ADMIN_PORT}/api/v1/auth/status" \
        "200" "$LOG_DIR/controller.log" || return 1

    # Positive probe: /api/v1/auth/status must report the token-only mode.
    echo "[4/5] Checking /api/v1/auth/status reports mode \"token\"..."
    local STATUS_BODY
    STATUS_BODY=$(curl -s "http://127.0.0.1:${NOAUTH_ADMIN_PORT}/api/v1/auth/status" 2>/dev/null || true)
    if ! echo "$STATUS_BODY" | grep -q '"mode":"token"'; then
        echo "FAIL [4/5]: /api/v1/auth/status did not report mode \"token\""
        echo "Response: $STATUS_BODY"
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    echo "PASS [4/5]: /api/v1/auth/status reports mode \"token\""

    # Business route: list HTTPRoute across all namespaces (namespaced kind).
    # Without a token registered the middleware returns 503 fail-close (not 401).
    assert_status "5/5" \
        "http://127.0.0.1:${NOAUTH_ADMIN_PORT}/api/v1/namespaced/HTTPRoute" \
        "503" "$LOG_DIR/controller.log" || return 1

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
    # conf_sync is mTLS by default: certs land under the work dir (TMPDIR) at the
    # work-dir relative paths the shared emitter writes into the config below.
    conf_sync_distribute_certs "$TMPDIR"

    cat > "$TMPDIR/controller.yaml" <<EOF
conf_center:
  type: file_system
  conf_dir: ${CONF_DIR}
EOF
    conf_sync_security_block server >> "$TMPDIR/controller.yaml"

    local CTRL_PID=""
    cleanup_phase2() {
        [ -n "$CTRL_PID" ] && kill "$CTRL_PID" 2>/dev/null || true
        [ -n "$CTRL_PID" ] && wait "$CTRL_PID" 2>/dev/null || true
        rm -rf "$TMPDIR"
        # Restore env to a clean state for any caller.
        unset EDGION_ADMIN_TOKEN
    }
    trap cleanup_phase2 EXIT

    # Env-var token — an obviously-fake test value, never a real secret.
    local ENVAUTH_ADMIN_TOKEN="test-admin-token-edgion-no-auth-integ-xyz"
    export EDGION_ADMIN_TOKEN="$ENVAUTH_ADMIN_TOKEN"

    precheck_port "$ENVAUTH_ADMIN_PORT" "controller-envauth admin" || return 1
    precheck_port "$ENVAUTH_GRPC_PORT"  "controller-envauth gRPC"  || return 1
    precheck_port "$ENVAUTH_PROBE_PORT" "controller-envauth probe" || return 1

    echo ""
    echo "=== Phase 2: Controller env-var auth startup ==="

    "$CONTROLLER_BIN" \
        -c "$TMPDIR/controller.yaml" \
        --work-dir "$TMPDIR" \
        --log-dir "$LOG_DIR" \
        --admin-listen "127.0.0.1:${ENVAUTH_ADMIN_PORT}" \
        --grpc-listen "127.0.0.1:${ENVAUTH_GRPC_PORT}" \
        --probe-listen "127.0.0.1:${ENVAUTH_PROBE_PORT}" \
        --metrics-listen "127.0.0.1:${ENVAUTH_METRICS_PORT}" \
        > "$LOG_DIR/controller.log" 2>&1 &
    CTRL_PID=$!

    # /health lives on the dedicated probe listener (probe port).
    if ! wait_for_health \
            "http://127.0.0.1:${ENVAUTH_PROBE_PORT}/health" \
            "$CTRL_PID" \
            "controller (env-auth)"; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi

    # The probe listener binds before the admin API, so a healthy probe does not
    # guarantee the admin API is accepting connections yet. Wait for the admin
    # API (unauthenticated /api/v1/auth/status returns 200) before logging in.
    if ! wait_for_url_200 \
            "http://127.0.0.1:${ENVAUTH_ADMIN_PORT}/api/v1/auth/status" \
            "$CTRL_PID" \
            "controller admin API (env-auth)"; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi

    # Primary assertion: a business route WITHOUT a Bearer header returns 401
    # (auth active), not 503 (no token registered). This proves the env-var token
    # synthesized an active provider (no [local_auth] section in the config) and
    # distinguishes the token-auth case from the no-auth case.
    echo "[6a/6] Verifying business route is 401 without a token..."
    assert_status "6a/6" \
        "http://127.0.0.1:${ENVAUTH_ADMIN_PORT}/api/v1/namespaced/HTTPRoute" \
        "401" "$LOG_DIR/controller.log" || return 1

    # Positive path: the same route WITH the correct pre-shared Bearer token → 200.
    echo "[6b/6] Verifying business route is 200 with the correct Bearer token..."
    local AUTHED_CODE
    AUTHED_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
        -H "Authorization: Bearer ${ENVAUTH_ADMIN_TOKEN}" \
        "http://127.0.0.1:${ENVAUTH_ADMIN_PORT}/api/v1/namespaced/HTTPRoute")
    if [ "$AUTHED_CODE" != "200" ]; then
        echo "FAIL [6b/6]: authed business route returned $AUTHED_CODE (expected 200)"
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    echo "PASS [6b/6]: authed business route returned 200"

    echo "PASS: Phase 2 (Controller token auth) complete"
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
