#!/usr/bin/env bash
# Own the short-TTL, lifetime-cap experiment outside the shared metrics process.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
UTILS_DIR="$PROJECT_ROOT/edgion-tests/integration/scripts/utils"
cd "$PROJECT_ROOT"
source "$UTILS_DIR/shell_compat.sh"
source "$UTILS_DIR/artifact_paths.sh"
source "$UTILS_DIR/conf_sync_certs.sh"
source "$UTILS_DIR/isolated_ports.sh"
if [ -z "${EDGION_TEST_BIN_DIR:-}" ]; then
    edgion_resolve_artifacts
fi
GATEWAY_BIN="${EDGION_GATEWAY_BIN:-$EDGION_TEST_BIN_DIR/edgion-gateway}"
CONTROLLER_BIN="${EDGION_CONTROLLER_BIN:-$EDGION_TEST_BIN_DIR/edgion-controller}"
TEST_CLIENT_BIN="${EDGION_TEST_CLIENT_BIN:-$EDGION_TEST_BIN_DIR/test_client}"
for binary in "$GATEWAY_BIN" "$CONTROLLER_BIN" "$TEST_CLIENT_BIN"; do
    if [ ! -x "$binary" ]; then
        echo "FAIL: required binary is not executable: $binary" >&2
        exit 1
    fi
done
for command in python3 openssl curl; do
    command -v "$command" >/dev/null || exit 1
done
isolated_load_ports metrics_lifetime
isolated_require_ports CTRL_ADMIN_PORT CTRL_GRPC_PORT CTRL_PROBE_PORT CTRL_METRICS_PORT \
    GW_ADMIN_PORT GW_PROBE_PORT GW_METRICS_PORT GW_HTTP_PORT BACKEND_HTTP_PORT
python3 "$UTILS_DIR/check_isolated_ports.py" --scenario metrics_lifetime

mkdir -p "$PROJECT_ROOT/tmp/integration_testing"
WORK_DIR=$(mktemp -d "$PROJECT_ROOT/tmp/integration_testing/metrics-lifetime.XXXXXX")
CTRL_DIR="$WORK_DIR/controller"
GW_DIR="$WORK_DIR/gateway"
CONF_DIR="$CTRL_DIR/conf"
mkdir -p "$CONF_DIR/cluster" "$CONF_DIR/edgion-test" "$CTRL_DIR/config" "$CTRL_DIR/logs" "$GW_DIR/logs"
echo "MetricsLifetime evidence: $WORK_DIR"
CTRL_PID=""
GW_PID=""
BACKEND_PID=""

stop_owned() {
    local pid="$1" label="$2" seconds="$3" status=0
    [ -n "$pid" ] || return 0
    if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" || status=$?
        echo "FAIL: owned $label exited before shutdown (status=$status)" >&2
        return 1
    fi
    kill -TERM "$pid" || return 1
    if ! edgion_wait_until "$seconds" 1 process_stopped "$pid"; then
        echo "FAIL: owned $label exceeded its shutdown deadline" >&2
        kill -KILL "$pid" 2>/dev/null || true
        wait "$pid" || true
        return 1
    fi
    wait "$pid" || status=$?
    case "$status" in
        0|143) return 0 ;; # Clean shutdown or the TERM requested above.
        *) echo "FAIL: owned $label shutdown status=$status" >&2; return 1 ;;
    esac
}
process_stopped() { ! kill -0 "$1" 2>/dev/null; }
cleanup() {
    # Pingora has a one-second drain, followed by the mandatory five-second
    # auxiliary grace window; leave bounded polling and process-exit margin.
    local status=$? gateway_wait=10
    trap - EXIT INT TERM
    if [ "${EDGION_COVERAGE:-0}" = 1 ]; then gateway_wait=60; fi
    # Keep the sync source alive until Gateway shutdown and coverage flush finish.
    stop_owned "$GW_PID" gateway "$gateway_wait" || status=1
    stop_owned "$CTRL_PID" controller 5 || status=1
    stop_owned "$BACKEND_PID" backend 5 || status=1
    echo "MetricsLifetime exit=$status; evidence retained at $WORK_DIR"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

ready_url() {
    kill -0 "$1" 2>/dev/null && curl --max-time 2 -fsS "$2" >/dev/null 2>&1
}
wait_ready() {
    if ! edgion_wait_until "$3" 1 ready_url "$1" "$2"; then
        echo "FAIL: readiness deadline: $2 (logs: $WORK_DIR)" >&2
        return 1
    fi
}
listener_ready() {
    kill -0 "$GW_PID" 2>/dev/null && python3 - "$GW_HTTP_PORT" <<'PY'
import socket
import sys
try:
    with socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=1):
        pass
except OSError:
    sys.exit(1)
PY
}

cp -R "$PROJECT_ROOT/config/crd" "$CTRL_DIR/config/crd"
cp "$PROJECT_ROOT/edgion-tests/integration/conf/base/GatewayClass.yaml" "$CONF_DIR/cluster/GatewayClass__public-gateway.yaml"
cat > "$CONF_DIR/cluster/Namespace__edgion-test.yaml" <<'YAML'
apiVersion: v1
kind: Namespace
metadata:
  name: edgion-test
YAML
cat > "$CONF_DIR/cluster/EdgionGatewayConfig__example-gateway.yaml" <<'YAML'
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: example-gateway
spec:
  securityProtect:
    allowLoopbackUpstream: true
YAML
# Gateway startup needs its listener before the scenario applies route fixtures.
cp "$PROJECT_ROOT/edgion-tests/integration/conf/HTTPRoute/MetricsLifetime/Gateway_default_metrics-lifetime.yaml" \
    "$CONF_DIR/edgion-test/Gateway_edgion-test_metrics-lifetime.yaml"
conf_sync_distribute_certs "$CTRL_DIR"
conf_sync_copy_certs "$CTRL_DIR" "$GW_DIR"
cat > "$CTRL_DIR/controller.yaml" <<YAML
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: $CONF_DIR
YAML
conf_sync_security_block server >> "$CTRL_DIR/controller.yaml"
cat > "$GW_DIR/gateway.yaml" <<YAML
work_dir: "."
logging:
  log_dir: "$GW_DIR/logs"
metrics:
  listen: "127.0.0.1:$GW_METRICS_PORT"
  backend_requests:
    labels:
      backend: true
      route: true
  cardinality_cap: 64
  request_duration:
    enabled: true
    idle_timeout_secs: 2
    labels:
      route_name: true
      route_namespace: true
YAML
conf_sync_security_block client >> "$GW_DIR/gateway.yaml"

# All owned admin clients and servers use the same explicit integration token.
export EDGION_ADMIN_TOKEN="${EDGION_TEST_ADMIN_TOKEN:-integration-metrics-lifetime-token-0123456789}"
export EDGION_TEST_ADMIN_TOKEN="$EDGION_ADMIN_TOKEN"
export EDGION_GATEWAY_ADMIN_TOKEN="$EDGION_ADMIN_TOKEN"
export EDGION_TOKEN="$EDGION_ADMIN_TOKEN"
export EDGION_TEST_ADMIN_API_URL="http://127.0.0.1:$CTRL_ADMIN_PORT"
export EDGION_TEST_GATEWAY_ADMIN_URL="http://127.0.0.1:$GW_ADMIN_PORT"
export EDGION_TEST_GATEWAY_METRICS_ENDPOINTS="http://127.0.0.1:$GW_METRICS_PORT"

python3 -u - "$BACKEND_HTTP_PORT" > "$WORK_DIR/backend.log" 2>&1 <<'PY' &
import http.server
import sys
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"metrics lifetime backend\n"
        self.send_response(200 if self.path == "/echo" else 404)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
class Server(http.server.HTTPServer):
    allow_reuse_address = True
Server(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
PY
BACKEND_PID=$!
wait_ready "$BACKEND_PID" "http://127.0.0.1:$BACKEND_HTTP_PORT/echo" 10
"$CONTROLLER_BIN" -c "$CTRL_DIR/controller.yaml" --work-dir "$CTRL_DIR" --log-dir "$CTRL_DIR/logs" \
    --admin-listen "127.0.0.1:$CTRL_ADMIN_PORT" --grpc-listen "127.0.0.1:$CTRL_GRPC_PORT" \
    --probe-listen "127.0.0.1:$CTRL_PROBE_PORT" --metrics-listen "127.0.0.1:$CTRL_METRICS_PORT" \
    > "$CTRL_DIR/logs/controller.log" 2>&1 &
CTRL_PID=$!
wait_ready "$CTRL_PID" "http://127.0.0.1:$CTRL_PROBE_PORT/ready" 90
wait_ready "$CTRL_PID" "http://127.0.0.1:$CTRL_METRICS_PORT/metrics" 30
env EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
    "$GATEWAY_BIN" -c "$GW_DIR/gateway.yaml" --work-dir "$GW_DIR" \
    --server-addr "https://127.0.0.1:$CTRL_GRPC_PORT" --integration-testing-mode \
    --admin-listen "127.0.0.1:$GW_ADMIN_PORT" --probe-listen "127.0.0.1:$GW_PROBE_PORT" \
    --grace-period 0 --graceful-shutdown-timeout 1 \
    > "$GW_DIR/logs/gateway.log" 2>&1 &
GW_PID=$!
wait_ready "$GW_PID" "http://127.0.0.1:$GW_PROBE_PORT/health" 60
wait_ready "$GW_PID" "http://127.0.0.1:$GW_METRICS_PORT/metrics" 30
edgion_wait_until 60 1 listener_ready
EDGION_TEST_METRICS_LIFETIME=1 "$TEST_CLIENT_BIN" -g --admin-port "$CTRL_ADMIN_PORT" --dynamic-scenario MetricsLifetime
echo "PASS: owned MetricsLifetime scenario"
