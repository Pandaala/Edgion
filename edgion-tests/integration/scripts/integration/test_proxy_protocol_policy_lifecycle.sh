#!/usr/bin/env bash
# Isolated file-system integration for inbound PROXY protocol references,
# startup fail-close, and restart-scoped runtime transitions.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
UTILS_DIR="${REPO_ROOT}/edgion-tests/integration/scripts/utils"
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

BIN_DIR="${REPO_ROOT}/target/debug"
GATEWAY_BIN="${EDGION_GATEWAY_BIN:-${BIN_DIR}/edgion-gateway}"
CONTROLLER_BIN="${EDGION_CONTROLLER_BIN:-${BIN_DIR}/edgion-controller}"
TEST_SERVER_BIN="${EDGION_TEST_SERVER_BIN:-${BIN_DIR}/test_server}"
CTRL_ADMIN_PORT=58920
CTRL_GRPC_PORT=58921
CTRL_PROBE_PORT=58922
GW_ADMIN_PORT=58923
GW_PROBE_PORT=58924
GW_METRICS_PORT=58925
CTRL_METRICS_PORT=58926
LISTENER_PORT=58930
TEST_SERVER_PORT=30063

for bin in "$GATEWAY_BIN" "$CONTROLLER_BIN" "$TEST_SERVER_BIN"; do
    [ -x "$bin" ] || { echo "ERROR: missing executable $bin" >&2; exit 1; }
done
for tool in curl nc lsof python3; do
    command -v "$tool" >/dev/null 2>&1 || { echo "ERROR: missing tool $tool" >&2; exit 1; }
done
if find "$REPO_ROOT/edgion-gateway/src" "$REPO_ROOT/edgion-resources/src" \
    -type f -newer "$GATEWAY_BIN" -print -quit | grep -q .; then
    echo "ERROR: $GATEWAY_BIN is older than Gateway/resource sources; rebuild before using this script as evidence" >&2
    exit 1
fi
if find "$REPO_ROOT/edgion-controller/src" "$REPO_ROOT/edgion-resources/src" \
    -type f -newer "$CONTROLLER_BIN" -print -quit | grep -q .; then
    echo "ERROR: $CONTROLLER_BIN is older than Controller/resource sources; rebuild before using this script as evidence" >&2
    exit 1
fi
if find "$REPO_ROOT/edgion-tests/src" -type f -newer "$TEST_SERVER_BIN" -print -quit | grep -q .; then
    echo "ERROR: $TEST_SERVER_BIN is older than integration-test sources; rebuild before using this script as evidence" >&2
    exit 1
fi

WORK_BASE="${REPO_ROOT}/tmp/integration_testing"
mkdir -p "$WORK_BASE"
WORK_DIR="$(mktemp -d "${WORK_BASE}/pp-policy-lifecycle.XXXXXX")"
CASE_DIR=""
CTRL_PID=""
GW_PID=""
TS_PID=""
KEEP_WORK=1

cleanup_processes() {
    local pid
    for pid in "$GW_PID" "$CTRL_PID" "$TS_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    sleep 1
    for pid in "$GW_PID" "$CTRL_PID" "$TS_PID"; do
        [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null || true
    done
    CTRL_PID=""; GW_PID=""; TS_PID=""
}
cleanup() {
    cleanup_processes
    if [ "$KEEP_WORK" -eq 0 ]; then rm -rf "$WORK_DIR"; else echo "INFO: kept $WORK_DIR" >&2; fi
}
trap cleanup EXIT

mkdir -p "$WORK_DIR/logs" "$WORK_DIR/config"
ln -s "$REPO_ROOT/config/crd" "$WORK_DIR/config/crd"
conf_sync_distribute_certs "$WORK_DIR"

wait_url() {
    local url="$1" pid="$2" want="$3" tries="${4:-100}" code i
    for i in $(seq 1 "$tries"); do
        code=$(curl -s -o /dev/null -w '%{http_code}' "$url" 2>/dev/null || true)
        [ "$code" = "$want" ] && return 0
        kill -0 "$pid" 2>/dev/null || return 1
        sleep 0.1
    done
    return 1
}

wait_tcp() {
    local host="$1" port="$2" pid="$3" tries="${4:-600}" i
    for i in $(seq 1 "$tries"); do
        nc -z "$host" "$port" 2>/dev/null && return 0
        kill -0 "$pid" 2>/dev/null || return 1
        sleep 0.1
    done
    echo "ERROR: ${host}:${port} not accepting within timeout" >&2
    return 1
}

# /ready is cache-ready, not listener-frozen. Writing a PROXY-policy change
# before Pingora preflight would let the update become the startup baseline
# and the process would never trip the runtime fatal guard.
wait_gateway_serving() {
    wait_url "http://127.0.0.1:${GW_PROBE_PORT}/ready" "$GW_PID" 200 600 || {
        tail -80 "$WORK_DIR/logs/gateway.log" >&2
        return 1
    }
    wait_tcp 127.0.0.1 "$LISTENER_PORT" "$GW_PID" 600 || {
        echo "ERROR: listener ${LISTENER_PORT} not bound after /ready" >&2
        tail -80 "$WORK_DIR/logs/gateway.log" >&2
        return 1
    }
}

start_test_server() {
    if lsof -iTCP:"$TEST_SERVER_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
        echo "ERROR: test_server port $TEST_SERVER_PORT already in use" >&2
        return 1
    fi
    "$TEST_SERVER_BIN" --http-ports "$TEST_SERVER_PORT" --log-level info \
        > "$WORK_DIR/logs/test_server.log" 2>&1 &
    TS_PID=$!
    wait_url "http://127.0.0.1:${TEST_SERVER_PORT}/health" "$TS_PID" 200 300 || {
        tail -80 "$WORK_DIR/logs/test_server.log" >&2
        return 1
    }
}

write_controller_config() {
    cat > "$WORK_DIR/config/controller.yaml" <<EOF
work_dir: "$WORK_DIR"
server:
  grpc_listen: "127.0.0.1:${CTRL_GRPC_PORT}"
  admin_listen: "127.0.0.1:${CTRL_ADMIN_PORT}"
  probe_listen: "127.0.0.1:${CTRL_PROBE_PORT}"
  metrics_listen: "127.0.0.1:${CTRL_METRICS_PORT}"
logging:
  log_dir: "$WORK_DIR/logs"
  log_prefix: edgion-controller
  log_level: info
  json_format: false
  console: false
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: "$CASE_DIR"
conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
EOF
    conf_sync_security_block server >> "$WORK_DIR/config/controller.yaml"
}

start_controller() {
    if nc -z 127.0.0.1 "$CTRL_METRICS_PORT" 2>/dev/null; then
        echo "ERROR: controller metrics port ${CTRL_METRICS_PORT} is already in use" >&2
        return 1
    fi
    "$CONTROLLER_BIN" -c "$WORK_DIR/config/controller.yaml" --work-dir "$WORK_DIR" \
        > "$WORK_DIR/logs/controller.log" 2>&1 &
    CTRL_PID=$!
    wait_url "http://127.0.0.1:${CTRL_PROBE_PORT}/ready" "$CTRL_PID" 200 600 || {
        tail -80 "$WORK_DIR/logs/controller.log" >&2; return 1;
    }
    wait_url "http://127.0.0.1:${CTRL_METRICS_PORT}/metrics" "$CTRL_PID" 200 300 || {
        tail -80 "$WORK_DIR/logs/controller.log" >&2; return 1;
    }
}

write_gateway_config() {
    cat > "$WORK_DIR/gateway.yaml" <<EOF
work_dir: "."
logging:
  log_dir: "$WORK_DIR/logs"
metrics:
  listen: "127.0.0.1:${GW_METRICS_PORT}"
EOF
    conf_sync_security_block client >> "$WORK_DIR/gateway.yaml"
}

launch_gateway() {
    : > "$WORK_DIR/logs/gateway.log"
    EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 "$GATEWAY_BIN" \
        -c "$WORK_DIR/gateway.yaml" --work-dir "$WORK_DIR" \
        --server-addr "https://127.0.0.1:${CTRL_GRPC_PORT}" --integration-testing-mode \
        --admin-listen "127.0.0.1:${GW_ADMIN_PORT}" --probe-listen "127.0.0.1:${GW_PROBE_PORT}" \
        > "$WORK_DIR/logs/gateway.log" 2>&1 &
    GW_PID=$!
}

condition_field() {
    local file="$1" want="$2" field="$3"
    [ -f "$file" ] || return 1
    awk -v want="$want" -v field="$field" '
      /^- / { reason=""; status="" }
      /^[[:space:]]*reason:/ { reason=$2; gsub(/["'\'' ]/, "", reason) }
      /^[[:space:]]*status:/ { status=$2; gsub(/["'\'' ]/, "", status) }
      /^[[:space:]]*type:/ {
        type=$2; gsub(/["'\'' ]/, "", type)
        if (type==want) { if (field=="reason") print reason; else print status; exit }
      }
    ' "$file"
}

wait_condition() {
    local file="$1" type="$2" status="$3" reason="$4" i got_status got_reason
    for i in $(seq 1 300); do
        got_status=$(condition_field "$file" "$type" status 2>/dev/null || true)
        got_reason=$(condition_field "$file" "$type" reason 2>/dev/null || true)
        if [ "$got_status" = "$status" ] && { [ -z "$reason" ] || [ "$got_reason" = "$reason" ]; }; then
            return 0
        fi
        sleep 0.1
    done
    echo "ERROR: $file $type expected $status/$reason, got $got_status/$got_reason" >&2
    return 1
}

new_case() {
    cleanup_processes
    CASE_DIR="$WORK_DIR/case-$1"
    mkdir -p "$CASE_DIR"
    write_controller_config
    cat > "$CASE_DIR/GatewayClass__edgion.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata: {name: edgion}
spec: {controllerName: edgion.io/gateway-controller}
EOF
}

write_ecd() {
    local namespace="$1" name="$2" visibility="$3" mode="$4"
    cat > "$CASE_DIR/EdgionConfigData_${namespace}_${name}.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata: {name: $name, namespace: $namespace}
spec:
  enable: true
  visibility: $visibility
  data:
    type: ProxyProtocolTrust
    config: {mode: $mode}
EOF
}

write_ecd_trusted() {
    local namespace="$1" name="$2" visibility="$3" output="$4"
    cat > "$output" <<EOF
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata: {name: $name, namespace: $namespace}
spec:
  enable: true
  visibility: $visibility
  data:
    type: ProxyProtocolTrust
    config: {mode: trustedSources, trustedCidrs: [127.0.0.1/32]}
EOF
}

write_ref_gateway() {
    local name="$1" target="$2" target_ns="$3" port="$4" protocol="${5:-TCP}"
    cat > "$CASE_DIR/Gateway_edge_${name}.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: $name
  namespace: edge
  annotations:
    edgion.io/inbound-proxy-protocol: '{"source":"resourceRef","resourceRef":{"name":"$target","namespace":"$target_ns"}}'
spec:
  gatewayClassName: edgion
  listeners: [{name: tcp, protocol: $protocol, port: $port}]
EOF
}

write_inline_gateway() {
    local name="$1" json="$2" port="$3" protocol="${4:-TCP}"
    cat > "$CASE_DIR/Gateway_edge_${name}.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: $name
  namespace: edge
  annotations:
    edgion.io/inbound-proxy-protocol: '$json'
spec:
  gatewayClassName: edgion
  listeners: [{name: tcp, protocol: $protocol, port: $port}]
EOF
}

write_http_route_resources() {
    local gateway_name="$1"
    cat > "$CASE_DIR/Service_edge_echo.yaml" <<EOF
apiVersion: v1
kind: Service
metadata: {name: echo, namespace: edge}
spec:
  ports: [{name: http, port: $TEST_SERVER_PORT, targetPort: $TEST_SERVER_PORT, protocol: TCP}]
EOF
    cat > "$CASE_DIR/EndpointSlice_edge_echo.yaml" <<EOF
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: echo
  namespace: edge
  labels: {kubernetes.io/service-name: echo}
addressType: IPv4
ports: [{name: http, port: $TEST_SERVER_PORT, protocol: TCP}]
endpoints:
  - addresses: ["127.0.0.1"]
    conditions: {ready: true}
EOF
    cat > "$CASE_DIR/HTTPRoute_edge_echo.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata: {name: echo, namespace: edge}
spec:
  parentRefs: [{name: $gateway_name, namespace: edge}]
  rules:
    - matches: [{path: {type: PathPrefix, value: /}}]
      backendRefs: [{name: echo, port: $TEST_SERVER_PORT}]
EOF
}

expect_startup_failure() {
    local code="$1" i rc=0
    launch_gateway
    for i in $(seq 1 300); do
        kill -0 "$GW_PID" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$GW_PID" 2>/dev/null; then
        echo "ERROR: Gateway served despite invalid startup case $code" >&2; return 1
    fi
    wait "$GW_PID" || rc=$?
    [ "$rc" -ne 0 ] || { echo "ERROR: invalid startup exited zero ($code)" >&2; return 1; }
    grep -q 'inbound PROXY protocol fatal' "$WORK_DIR/logs/gateway.log" || {
        tail -80 "$WORK_DIR/logs/gateway.log" >&2; return 1;
    }
    grep -Eq "$code" "$WORK_DIR/logs/gateway.log" || {
        tail -80 "$WORK_DIR/logs/gateway.log" >&2; return 1;
    }
    if grep -Eq 'trustedCidrs|"source"[[:space:]]*:|\\"source\\"[[:space:]]*:' \
        "$WORK_DIR/logs/gateway.log"; then
        echo "ERROR: raw policy leaked into startup diagnostic" >&2; return 1
    fi
    GW_PID=""
}

test_reference_matrix() {
    new_case refs
    write_ecd trust cluster Cluster required
    write_ecd trust namespace Namespace required
    write_ecd trust exact Namespace required
    write_ecd trust wrong-group Namespace required
    write_ecd trust wrong-kind Namespace required
    write_ecd trust wrong-name Namespace required
    write_ref_gateway cluster-ok cluster trust 58631
    write_ref_gateway namespace-denied namespace trust 58632
    write_ref_gateway exact-ok exact trust 58633
    write_ref_gateway wrong-group wrong-group trust 58634
    write_ref_gateway wrong-kind wrong-kind trust 58635
    write_ref_gateway wrong-name wrong-name trust 58636
    cat > "$CASE_DIR/ReferenceGrant_trust_exact.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata: {name: exact, namespace: trust}
spec:
  from:
    - {group: gateway.networking.k8s.io, kind: Gateway, namespace: edge}
  to:
    - {group: edgion.io, kind: EdgionConfigData, name: exact}
EOF
    cat > "$CASE_DIR/ReferenceGrant_trust_wrong-group.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata: {name: wrong-group, namespace: trust}
spec:
  from:
    - {group: edgion.io, kind: Gateway, namespace: edge}
  to:
    - {group: edgion.io, kind: EdgionConfigData, name: wrong-group}
EOF
    cat > "$CASE_DIR/ReferenceGrant_trust_wrong-kind.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata: {name: wrong-kind, namespace: trust}
spec:
  from:
    - {group: gateway.networking.k8s.io, kind: HTTPRoute, namespace: edge}
  to:
    - {group: edgion.io, kind: EdgionConfigData, name: wrong-kind}
EOF
    cat > "$CASE_DIR/ReferenceGrant_trust_wrong-name.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata: {name: wrong-name, namespace: trust}
spec:
  from:
    - {group: gateway.networking.k8s.io, kind: Gateway, namespace: edge}
  to:
    - {group: edgion.io, kind: EdgionConfigData, name: another-name}
EOF
    start_controller
    wait_condition "$CASE_DIR/Gateway_edge_cluster-ok.yaml.status" ResolvedRefs True ResolvedRefs
    wait_condition "$CASE_DIR/Gateway_edge_exact-ok.yaml.status" ResolvedRefs True ResolvedRefs
    for name in namespace-denied wrong-group wrong-kind wrong-name; do
        wait_condition "$CASE_DIR/Gateway_edge_${name}.yaml.status" ResolvedRefs False RefNotPermitted
    done
}

run_invalid_case() {
    local name="$1" expected_status="$2" expected_reason="$3" diagnostic="$4"
    new_case "$name"
    case "$name" in
        malformed) write_inline_gateway "$name" '{' "$LISTENER_PORT" ;;
        empty) write_inline_gateway "$name" '{"source":"inline","config":{"mode":"trustedSources","trustedCidrs":[]}}' "$LISTENER_PORT" ;;
        match-all) write_inline_gateway "$name" '{"source":"inline","config":{"mode":"trustedSources","trustedCidrs":["0.0.0.0/0"]}}' "$LISTENER_PORT" ;;
        missing) write_ref_gateway "$name" absent edge "$LISTENER_PORT" ;;
        denied)
            write_ecd trust denied Namespace required
            write_ref_gateway "$name" denied trust "$LISTENER_PORT"
            ;;
        shared)
            write_inline_gateway shared-required '{"source":"inline","config":{"mode":"required"}}' "$LISTENER_PORT"
            write_inline_gateway shared-trusted '{"source":"inline","config":{"mode":"trustedSources","trustedCidrs":["127.0.0.1"]}}' "$LISTENER_PORT"
            ;;
    esac
    start_controller
    if [ "$name" = shared ]; then
        wait_condition "$CASE_DIR/Gateway_edge_shared-required.yaml.status" Conflicted True ProxyProtocolPolicyConflict
    else
        wait_condition "$CASE_DIR/Gateway_edge_${name}.yaml.status" "$expected_status" False "$expected_reason"
    fi
    expect_startup_failure "$diagnostic"
}

send_required_request() {
    python3 - "$LISTENER_PORT" <<'PY'
import socket, sys
port=int(sys.argv[1]); s=socket.create_connection(("127.0.0.1", port), 2)
s.sendall(f"PROXY TCP4 203.0.113.9 127.0.0.1 40000 {port}\r\nGET /headers HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n".encode())
s.settimeout(2)
chunks=[]
try:
    while True:
        data=s.recv(65536)
        if not data: break
        chunks.append(data)
except Exception: pass
s.close(); print(b"".join(chunks).decode("latin1", "replace"))
PY
}

send_plain_request() {
    local source_ip="${1:-127.0.0.1}"
    python3 - "$LISTENER_PORT" "$source_ip" <<'PY'
import socket, sys
port, source_ip = int(sys.argv[1]), sys.argv[2]
s=socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.settimeout(2)
try:
    s.bind((source_ip, 0))
    s.connect(("127.0.0.1", port))
except OSError as exc:
    print(f"ERROR: cannot establish probe from explicit source {source_ip}: {exc}", file=sys.stderr)
    s.close()
    raise SystemExit(2)
s.sendall(b"GET /headers HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
s.settimeout(2)
chunks=[]
try:
    while True:
        data=s.recv(65536)
        if not data: break
        chunks.append(data)
except Exception: pass
s.close(); print(b"".join(chunks).decode("latin1", "replace"))
PY
}

send_trusted_proxy_request() {
    local version="$1"
    python3 - "$LISTENER_PORT" "$version" <<'PY'
import socket, struct, sys
port, version = int(sys.argv[1]), sys.argv[2]
advertised = "203.0.113.9"
request = b"GET /headers HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"
if version == "v1":
    prefix = f"PROXY TCP4 {advertised} 127.0.0.1 40000 {port}\r\n".encode()
elif version == "v2":
    addresses = (
        socket.inet_aton(advertised)
        + socket.inet_aton("127.0.0.1")
        + struct.pack("!HH", 40000, port)
    )
    prefix = (
        b"\r\n\r\n\x00\r\nQUIT\n"
        + bytes((0x21, 0x11))
        + struct.pack("!H", len(addresses))
        + addresses
    )
else:
    print(f"ERROR: unsupported PROXY probe version {version}", file=sys.stderr)
    raise SystemExit(2)
s=socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.settimeout(2)
try:
    s.bind(("127.0.0.1", 0))
    s.connect(("127.0.0.1", port))
except OSError as exc:
    print(f"ERROR: cannot establish trusted {version} probe from 127.0.0.1: {exc}", file=sys.stderr)
    s.close()
    raise SystemExit(2)
s.sendall(prefix + request)
chunks=[]
try:
    while True:
        data=s.recv(65536)
        if not data: break
        chunks.append(data)
except socket.timeout:
    pass
s.close(); print(b"".join(chunks).decode("latin1", "replace"))
PY
}

assert_http_200_with_identity() {
    local response="$1" identity="$2" label="$3" status_line
    status_line=$(printf '%s' "$response" | sed -n '1{s/\r$//;p;}')
    case "$status_line" in
        "HTTP/1.0 200"*|"HTTP/1.1 200"*) ;;
        *) echo "ERROR: $label expected HTTP 200 from /headers, got '$status_line'" >&2; return 1 ;;
    esac
    case "$response" in
        *"$identity"*) ;;
        *) echo "ERROR: $label response did not contain client identity $identity" >&2; return 1 ;;
    esac
}

assert_required_policy() {
    local response plain
    response=$(send_required_request)
    assert_http_200_with_identity "$response" 203.0.113.9 "required PROXY request"
    plain=$(send_plain_request)
    case "$plain" in
        HTTP/*) echo "ERROR: required policy served a plain request without a PROXY header" >&2; return 1 ;;
        *) kill -0 "$GW_PID" 2>/dev/null || { echo "ERROR: Gateway exited during required plain-request rejection proof" >&2; return 1; } ;;
    esac
}

assert_trusted_sources_policy() {
    local version response plain
    for version in v1 v2; do
        if ! response=$(send_trusted_proxy_request "$version"); then
            echo "ERROR: trustedSources topology could not prove trusted $version input" >&2
            return 1
        fi
        assert_http_200_with_identity "$response" 203.0.113.9 \
            "trustedSources trusted $version PROXY request"
    done
    if ! plain=$(send_plain_request 127.0.0.2); then
        echo "ERROR: trustedSources topology cannot prove a source outside 127.0.0.1/32" >&2
        return 1
    fi
    assert_http_200_with_identity "$plain" 127.0.0.2 \
        "trustedSources untrusted direct request"
    case "$plain" in
        *203.0.113.9*)
            echo "ERROR: untrusted direct request acquired the advertised PROXY identity" >&2
            return 1
            ;;
    esac
}

probe_listener_http() {
    python3 - "$LISTENER_PORT" <<'PY'
import socket, sys
port = int(sys.argv[1])
sock = socket.socket()
sock.settimeout(0.3)
try:
    sock.connect(("127.0.0.1", port))
    sock.sendall(b"GET /headers HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
    data = sock.recv(64)
except Exception:
    data = b""
sock.close()
sys.exit(0 if data.startswith(b"HTTP/") else 1)
PY
}

wait_runtime_fatal() {
    local saw_unready=0 saw_reject=0 i code rc=0
    # Short I/O timeouts are load-bearing: a 2s PROXY probe can span the entire
    # FastShutdown window and miss both the readiness flip and the accept-filter
    # rejection even though the process did trip ListenerPolicyTransition.
    # FastShutdown makes /ready unreachable (curl 000) rather than returning 503
    # while the process is still draining; both mean the probe stopped succeeding.
    for i in $(seq 1 400); do
        code=$(curl -s -m 0.3 -o /dev/null -w '%{http_code}' \
            "http://127.0.0.1:${GW_PROBE_PORT}/ready" 2>/dev/null || true)
        if [ "$code" != 200 ]; then
            saw_unready=1
        fi
        if ! kill -0 "$GW_PID" 2>/dev/null; then
            break
        fi
        if ! probe_listener_http; then
            saw_reject=1
        fi
        sleep 0.05
    done
    kill -0 "$GW_PID" 2>/dev/null && { echo "ERROR: Gateway did not exit after policy transition" >&2; return 1; }
    wait "$GW_PID" || rc=$?
    [ "$rc" -ne 0 ] || { echo "ERROR: runtime policy fatal exited zero" >&2; return 1; }
    grep -Eq 'ListenerPolicyTransition|stopping the gateway' \
        "$WORK_DIR/logs/gateway.log" "$WORK_DIR/logs/edgion-gateway.log" || {
        echo "ERROR: runtime fatal log was not observed" >&2
        tail -80 "$WORK_DIR/logs/gateway.log" >&2
        return 1
    }
    [ "$saw_unready" -eq 1 ] || { echo "ERROR: readiness did not stop succeeding after policy transition" >&2; return 1; }
    [ "$saw_reject" -eq 1 ] || { echo "ERROR: new TCP rejection was not observed" >&2; return 1; }
    GW_PID=""
}

test_runtime_transitions() {
    new_case runtime-inline
    start_test_server
    cat > "$CASE_DIR/Gateway_edge_runtime.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata: {name: runtime, namespace: edge}
spec:
  gatewayClassName: edgion
  listeners: [{name: tcp, protocol: HTTP, port: $LISTENER_PORT}]
EOF
    write_http_route_resources runtime
    start_controller; launch_gateway
    wait_gateway_serving
    plain=$(send_plain_request)
    assert_http_200_with_identity "$plain" 127.0.0.1 "disabled baseline before transition"
    write_inline_gateway runtime '{"source":"inline","config":{"mode":"required"}}' "$LISTENER_PORT" HTTP
    wait_runtime_fatal
    launch_gateway; wait_gateway_serving
    assert_required_policy
    cleanup_processes

    new_case runtime-ref
    start_test_server
    write_ecd edge runtime-policy Namespace required
    write_ref_gateway runtime runtime-policy edge "$LISTENER_PORT" HTTP
    write_http_route_resources runtime
    start_controller; launch_gateway
    wait_gateway_serving
    assert_required_policy
    write_ecd_trusted edge runtime-policy Namespace "$CASE_DIR/runtime-policy.tmp"
    mv "$CASE_DIR/runtime-policy.tmp" "$CASE_DIR/EdgionConfigData_edge_runtime-policy.yaml"
    wait_runtime_fatal
    launch_gateway; wait_gateway_serving
    assert_trusted_sources_policy
    rm "$CASE_DIR/EdgionConfigData_edge_runtime-policy.yaml"
    wait_runtime_fatal
    expect_startup_failure 'ReferenceNotFound'
    write_ecd edge runtime-policy Namespace required
    wait_condition "$CASE_DIR/Gateway_edge_runtime.yaml.status" ResolvedRefs True ResolvedRefs
    launch_gateway; wait_gateway_serving
    assert_required_policy
}

write_gateway_config
test_reference_matrix
run_invalid_case malformed Accepted InvalidProxyProtocolConfig 'MalformedJson'
run_invalid_case empty Accepted InvalidProxyProtocolConfig 'InvalidPolicy'
run_invalid_case match-all Accepted InvalidProxyProtocolConfig 'InvalidPolicy'
run_invalid_case missing ResolvedRefs InvalidProxyProtocolRef 'ReferenceNotFound'
run_invalid_case denied ResolvedRefs RefNotPermitted 'ReferenceNotPermitted'
run_invalid_case shared Conflicted ProxyProtocolPolicyConflict 'SharedListenerConflict|SharedListenerPolicyConflict'
test_runtime_transitions
KEEP_WORK=0
echo "PASS: inbound PROXY protocol policy lifecycle"
