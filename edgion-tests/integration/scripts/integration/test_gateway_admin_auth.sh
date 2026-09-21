#!/usr/bin/env bash
# Structural regression for Gateway admin API authentication.
#
# The Gateway admin API (default 127.0.0.1:12001) is guarded by a static bearer
# token (src/core/gateway/api/admin_auth.rs). This test asserts the four corners
# of that contract against a real gateway process started on loopback:
#
#   1. No token configured (no EDGION_GATEWAY_ADMIN_TOKEN), integration-testing
#      mode ON  -> GET /configclient/gateway/list returns 503 (fail-close), and
#      the startup log contains the no-token WARN.
#   2. Token configured, NO Authorization header  -> 401.
#   3. Token configured, correct Authorization: Bearer <token>  -> 200.
#   4. Probe port (12000) /health and /ready  -> 200 WITHOUT any token
#      (the probe router is auth-free by design).
#
# Boot model (mirrors test_controller_no_auth.sh's boot/curl/assert/cleanup
# skeleton, trap-based teardown, and status-capture helper):
#   The gateway is NOT standalone — its admin + probe HTTP servers only spawn
#   after it connects to a Controller gRPC endpoint and fetches server info. So
#   this test first boots one minimal Controller (file_system conf center, no
#   resources) purely as the gRPC sync backend, then launches the gateway twice
#   against it: once without a token (Phase 1), once with a token (Phase 2). The
#   gateway↔controller gRPC config-sync path is independent of the controller's
#   HTTP admin auth, so the controller needs no credentials here.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
UTILS_DIR="${REPO_ROOT}/edgion-tests/integration/scripts/utils"
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

GATEWAY_BIN="${EDGION_GATEWAY_BIN:-$REPO_ROOT/target/debug/edgion-gateway}"
CONTROLLER_BIN="${EDGION_CONTROLLER_BIN:-$REPO_ROOT/target/debug/edgion-controller}"

# A token >= 32 chars so the gateway does not also emit the weak-token WARN.
TEST_TOKEN="integration-gateway-admin-token-0123456789"

# Ports chosen to avoid the main integration stack (12001 admin / 12000 probe /
# 12090 metrics / 12101 controller-admin / 12151 controller-grpc) and the other
# no-auth preflight tests (58xxx).
GW_ADMIN_PORT=58300
GW_PROBE_PORT=58301
GW_METRICS_PORT=58302
CTRL_ADMIN_PORT=58303
CTRL_GRPC_PORT=58304
CTRL_PROBE_PORT=58305
CTRL_METRICS_PORT=58307
# Data-plane listener port for the seeded Gateway. The gateway binds this when it
# configures listeners during startup; it must be free but is never connected to.
GW_DATAPLANE_PORT=58306

# =============================================================================
# Prerequisites
# =============================================================================
if [ ! -x "$GATEWAY_BIN" ]; then
    echo "ERROR: edgion-gateway binary not found at $GATEWAY_BIN"
    echo "  Run: cargo build --bin edgion-gateway"
    exit 1
fi
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

# Poll a TCP port until it accepts a connection or the timeout is reached.
# Watches the process: if it exits early we abort.
wait_for_tcp_port() {
    local host="$1" port="$2" pid="$3" label="$4" tries="${5:-60}"
    local i
    for i in $(seq 1 "$tries"); do
        if nc -z "$host" "$port" 2>/dev/null; then
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "FAIL: $label exited during startup"
            return 1
        fi
        sleep 1
    done
    echo "FAIL: $label ${host}:${port} did not accept a connection within ${tries}s"
    return 1
}

# Poll a URL until it returns 200 or the timeout is reached. Watches the process:
# if it exits early we abort.
wait_for_url_200() {
    local url="$1" pid="$2" label="$3" tries="${4:-60}"
    local i code
    for i in $(seq 1 "$tries"); do
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
    echo "FAIL: $label $url did not return 200 within ${tries}s"
    return 1
}

# Status-only curl helper (per the task contract).
http_status() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

# =============================================================================
# Shared minimal Controller (gRPC sync backend for the gateway)
# =============================================================================
CTRL_TMPDIR=""
CTRL_PID=""

start_controller() {
    CTRL_TMPDIR=$(mktemp -d)
    local CONFIG_DIR="${CTRL_TMPDIR}/config"
    local LOG_DIR="${CTRL_TMPDIR}/logs"
    local CONF_DIR="${CTRL_TMPDIR}/conf"
    mkdir -p "$CONFIG_DIR" "$LOG_DIR" "$CONF_DIR"

    # The Controller exits in file_system mode if config/crd is absent or empty.
    if [ -d "${REPO_ROOT}/config/crd" ]; then
        cp -R "${REPO_ROOT}/config/crd/." "${CONFIG_DIR}/crd/"
    else
        echo "FAIL: CRD schema source not found: ${REPO_ROOT}/config/crd"
        return 1
    fi

    # Seed a minimal Gateway topology into the file_system conf center BEFORE the
    # controller starts (its watcher loads pre-existing files during init). The
    # gateway exits at startup if zero Gateway resources sync from the controller
    # (src/core/gateway/runtime/server/base.rs), so a GatewayClass +
    # EdgionGatewayConfig + one Gateway are the minimum to let it fully start and
    # spawn its admin/probe servers. Layout (see file_system storage.rs):
    #   cluster-scoped:   <root>/cluster/Kind__name.yaml
    #   namespace-scoped: <root>/<ns>/Kind_ns_name.yaml
    mkdir -p "${CONF_DIR}/cluster" "${CONF_DIR}/edgion-test"
    local base="${REPO_ROOT}/edgion-tests/integration/conf/base"
    if [ ! -f "${base}/GatewayClass.yaml" ] || [ ! -f "${base}/EdgionGatewayConfig.yaml" ]; then
        echo "FAIL: base Gateway resources not found under ${base}"
        return 1
    fi
    cp "${base}/GatewayClass.yaml" "${CONF_DIR}/cluster/GatewayClass__public-gateway.yaml"
    cp "${base}/EdgionGatewayConfig.yaml" "${CONF_DIR}/cluster/EdgionGatewayConfig__example-gateway.yaml"
    cat > "${CONF_DIR}/edgion-test/Gateway_edgion-test_admin-auth-gw.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: admin-auth-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: http
      protocol: HTTP
      port: ${GW_DATAPLANE_PORT}
      allowedRoutes:
        namespaces:
          from: All
EOF

    # Minimal controller config — file_system conf center seeded above. No
    # [local_auth] / [auth]: the gateway↔controller gRPC sync path does not use
    # the admin HTTP auth, and this test never calls the controller's admin API.
    # conf_sync is mTLS by default: distribute the certs into the Controller's work
    # dir (CTRL_TMPDIR), then let the shared emitter write the transport block.
    conf_sync_distribute_certs "$CTRL_TMPDIR"

    cat > "$CTRL_TMPDIR/controller.yaml" <<EOF
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: ${CONF_DIR}
EOF
    conf_sync_security_block server >> "$CTRL_TMPDIR/controller.yaml"

    precheck_port "$CTRL_ADMIN_PORT" "controller admin" || return 1
    precheck_port "$CTRL_GRPC_PORT"  "controller gRPC"  || return 1
    precheck_port "$CTRL_PROBE_PORT" "controller probe" || return 1
    precheck_port "$CTRL_METRICS_PORT" "controller metrics" || return 1

    echo "=== Starting backend Controller (gRPC sync source) ==="
    "$CONTROLLER_BIN" \
        -c "$CTRL_TMPDIR/controller.yaml" \
        --work-dir "$CTRL_TMPDIR" \
        --log-dir "$LOG_DIR" \
        --admin-listen "127.0.0.1:${CTRL_ADMIN_PORT}" \
        --grpc-listen "127.0.0.1:${CTRL_GRPC_PORT}" \
        --probe-listen "127.0.0.1:${CTRL_PROBE_PORT}" \
        --metrics-listen "127.0.0.1:${CTRL_METRICS_PORT}" \
        > "$LOG_DIR/controller.log" 2>&1 &
    CTRL_PID=$!

    # The gateway needs the controller's gRPC config-sync server to be ready (not
    # merely bound) before its first get_server_info call, otherwise the gateway
    # exits with "ConfigSyncServer not ready". The controller's probe /ready flips
    # to 200 once ConfMgr init completes, which is the readiness we depend on.
    if ! wait_for_url_200 \
            "http://127.0.0.1:${CTRL_PROBE_PORT}/ready" \
            "$CTRL_PID" \
            "controller /ready" 90; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    wait_for_url_200 \
        "http://127.0.0.1:${CTRL_METRICS_PORT}/metrics" \
        "$CTRL_PID" "controller metrics" 30 || return 1
    echo "PASS: backend Controller is ready (gRPC on 127.0.0.1:${CTRL_GRPC_PORT})"
}

# =============================================================================
# Gateway launch helper
#
# Launches the gateway against the shared controller. The admin token is taken
# from the (already-exported-or-unset) EDGION_GATEWAY_ADMIN_TOKEN env var, so the
# caller controls Phase 1 (unset) vs Phase 2 (set) by toggling that var before
# calling. Echoes the gateway PID and log path via the GW_PID / GW_LOG globals.
# =============================================================================
GW_TMPDIR=""
GW_PID=""
GW_LOG=""

start_gateway() {
    local label="$1"
    GW_TMPDIR=$(mktemp -d)
    local LOG_DIR="${GW_TMPDIR}/logs"
    mkdir -p "$LOG_DIR"
    GW_LOG="$LOG_DIR/gateway.log"

    # Minimal gateway config:
    #   - logging.log_dir MUST be set to a writable path; its default is
    #     /usr/local/edgion/logs, which is not writable in a test sandbox and
    #     makes init_logging fail with "Permission denied" before any output.
    #   - metrics.listen is moved off the 12090 default so the two sequential
    #     launches (and the host) cannot collide on it.
    # admin/probe listens come from CLI flags below.
    # conf_sync is mTLS by default: the Gateway needs the CLIENT half of the SAME
    # cert set the Controller serves (one CA), in its own work dir (GW_TMPDIR,
    # passed as --work-dir below) — copy from the Controller's work dir instead
    # of generating an unrelated set.
    conf_sync_copy_certs "$CTRL_TMPDIR" "$GW_TMPDIR"

    cat > "$GW_TMPDIR/gateway.yaml" <<EOF
work_dir: "."
logging:
  log_dir: "${LOG_DIR}"
metrics:
  listen: "127.0.0.1:${GW_METRICS_PORT}"
EOF
    conf_sync_security_block client >> "$GW_TMPDIR/gateway.yaml"

    precheck_port "$GW_ADMIN_PORT"     "gateway admin"     || return 1
    precheck_port "$GW_PROBE_PORT"     "gateway probe"     || return 1
    precheck_port "$GW_METRICS_PORT"   "gateway metrics"   || return 1
    precheck_port "$GW_DATAPLANE_PORT" "gateway dataplane" || return 1

    echo "=== Starting gateway: ${label} ==="
    RUST_LOG="info,pingora_proxy=error,pingora_core=error" \
    EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
    "$GATEWAY_BIN" \
        -c "$GW_TMPDIR/gateway.yaml" \
        --work-dir "$GW_TMPDIR" \
        --server-addr "https://127.0.0.1:${CTRL_GRPC_PORT}" \
        --integration-testing-mode \
        --admin-listen "127.0.0.1:${GW_ADMIN_PORT}" \
        --probe-listen "127.0.0.1:${GW_PROBE_PORT}" \
        > "$GW_LOG" 2>&1 &
    GW_PID=$!

    # The admin + probe HTTP servers spawn together (start_auxiliary_services).
    # /health returns 200 unconditionally once the probe server is bound, which
    # proves both servers are listening (the admin server binds in the same step).
    if ! wait_for_url_200 \
            "http://127.0.0.1:${GW_PROBE_PORT}/health" \
            "$GW_PID" \
            "gateway ${label} (/health)" 60; then
        echo "---- gateway log ----"
        cat "$GW_LOG"
        return 1
    fi
    echo "PASS: gateway ${label} admin+probe servers are listening"
}

stop_gateway() {
    [ -n "$GW_PID" ] && kill "$GW_PID" 2>/dev/null || true
    [ -n "$GW_PID" ] && wait "$GW_PID" 2>/dev/null || true
    [ -n "$GW_TMPDIR" ] && rm -rf "$GW_TMPDIR" || true
    GW_PID=""
    GW_TMPDIR=""
}

# =============================================================================
# Global cleanup (single trap; stops gateway then controller).
# =============================================================================
cleanup() {
    stop_gateway
    [ -n "$CTRL_PID" ] && kill "$CTRL_PID" 2>/dev/null || true
    [ -n "$CTRL_PID" ] && wait "$CTRL_PID" 2>/dev/null || true
    [ -n "$CTRL_TMPDIR" ] && rm -rf "$CTRL_TMPDIR" || true
}
trap cleanup EXIT

# =============================================================================
# Phase 1: no token configured -> 503 fail-close + startup WARN
# =============================================================================
run_phase1() {
    echo ""
    echo "=== Phase 1: gateway admin auth with NO token configured ==="

    # Ensure the token is NOT set so the admin API fails closed.
    unset EDGION_GATEWAY_ADMIN_TOKEN

    start_gateway "no-token" || return 1

    # [1/4] Business admin route must fail-close with 503 (no token configured).
    local code
    code=$(http_status "http://127.0.0.1:${GW_ADMIN_PORT}/configclient/gateway/list")
    if [ "$code" != "503" ]; then
        echo "FAIL [1/4]: /configclient/gateway/list returned $code (expected 503 fail-close)"
        cat "$GW_LOG"
        return 1
    fi
    echo "PASS [1/4]: unconfigured admin token -> /configclient/gateway/list = 503"

    # Startup WARN must be present (event=no_admin_token).
    if ! grep -q "no_admin_token\|No gateway admin token configured" "$GW_LOG"; then
        echo "FAIL [1/4]: expected no-token startup WARN not found in gateway log"
        cat "$GW_LOG"
        return 1
    fi
    echo "PASS [1/4]: no-token startup WARN present in gateway log"

    stop_gateway
}

# =============================================================================
# Phase 2: token configured -> 401 (no header) / 200 (correct) + probe open
# =============================================================================
run_phase2() {
    echo ""
    echo "=== Phase 2: gateway admin auth WITH token configured ==="

    export EDGION_GATEWAY_ADMIN_TOKEN="$TEST_TOKEN"

    start_gateway "with-token" || return 1

    # [2/4] No Authorization header -> 401.
    local code
    code=$(http_status "http://127.0.0.1:${GW_ADMIN_PORT}/configclient/gateway/list")
    if [ "$code" != "401" ]; then
        echo "FAIL [2/4]: no-header admin call returned $code (expected 401)"
        cat "$GW_LOG"
        return 1
    fi
    echo "PASS [2/4]: token configured, no header -> 401"

    # [3/4] Correct bearer token -> 200.
    code=$(http_status \
        -H "Authorization: Bearer ${TEST_TOKEN}" \
        "http://127.0.0.1:${GW_ADMIN_PORT}/configclient/gateway/list")
    if [ "$code" != "200" ]; then
        echo "FAIL [3/4]: correct-token admin call returned $code (expected 200)"
        cat "$GW_LOG"
        return 1
    fi
    echo "PASS [3/4]: token configured, correct Bearer token -> 200"

    # [4/4] Probe port is auth-free: /health and /ready must be 200 WITHOUT a token.
    code=$(http_status "http://127.0.0.1:${GW_PROBE_PORT}/health")
    if [ "$code" != "200" ]; then
        echo "FAIL [4/4]: probe /health (no token) returned $code (expected 200)"
        cat "$GW_LOG"
        return 1
    fi
    # /ready returns 200 only once all caches have synced from the controller.
    # With an empty controller conf the initial (empty) snapshots still mark every
    # watched cache ready, so poll briefly for it.
    if ! wait_for_url_200 \
            "http://127.0.0.1:${GW_PROBE_PORT}/ready" \
            "$GW_PID" \
            "gateway probe /ready" 60; then
        cat "$GW_LOG"
        return 1
    fi
    echo "PASS [4/4]: probe /health and /ready (no token) -> 200"

    stop_gateway
}

# =============================================================================
# Entry point
# =============================================================================
main() {
    start_controller
    run_phase1
    run_phase2
    echo ""
    echo "PASS: gateway admin auth (503 unconfigured / 401 / 200 / probe open)"
}

main
