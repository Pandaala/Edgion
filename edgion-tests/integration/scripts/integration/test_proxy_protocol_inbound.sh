#!/usr/bin/env bash
# =============================================================================
# Inbound PROXY protocol (v1 + v2) — end-to-end integration test.
#
# Contract: docs/en/dev-guide/annotations-guide.md
#
# WHY STANDALONE (cannot join the shared run_integration.sh stack):
#   A listener selected by `edgion.io/inbound-proxy-protocol` in `required` mode
#   REJECTS every connection
#   that does not begin with a PROXY header (HAProxy `accept-proxy` semantics),
#   so the listener is unusable by any other suite sharing the stack. The flag is
#   also read once at listener-bind time — there is no hot-rebind path — so it
#   cannot be changed while serving. This script therefore boots its own minimal
#   stack (test_server + file_system controller + gateway) on dedicated 584xx
#   ports, mirroring test_plugin_policy.sh.
#
# WHY DOCKER, AND WHY HAPROXY FOR BOTH VERSIONS:
#   The point of this test is that a REAL load balancer's header is parsed, not a
#   byte string we assembled ourselves — the byte level is already covered by unit
#   tests in the pingora fork. HAProxy is the reference implementation (the spec
#   is a HAProxy document) and emits both versions from one binary, so the v1 and
#   v2 legs share sender, image, and topology and differ in exactly one variable:
#   `send-proxy` vs `send-proxy-v2`. A failure on either leg is therefore
#   attributable to the protocol version alone. nginx is deliberately NOT used:
#   it can only ever send v1 (it receives v2 but has never implemented emitting
#   it — verified empirically), so it could not cover the v2 leg at all, and
#   mixing senders would make the legs differ by more than the version.
#
# TOPOLOGY (a private docker network makes the assertion unambiguous):
#
#     client container 172.28.0.100
#        ├─→ haproxy 172.28.0.10:58440  --send-proxy-v2--┐
#        └─→ haproxy 172.28.0.10:58441  --send-proxy-----┤ (v1)
#                                                        ↓
#                                   host.docker.internal:28430  (gateway, PP on)
#                                                        ↓
#                                          test_server :30061 /headers
#
#   The client's address is pinned to 172.28.0.100, an address the gateway can
#   never observe as a raw TCP peer (the proxies reach the host through NAT). So
#   seeing 172.28.0.100 in the echoed X-Forwarded-For proves the address came
#   from the PROXY header and nothing else.
#
# PHASES:
#   1. via HAProxy (PPv2)  → XFF/X-Real-IP == 172.28.0.100
#   2. via HAProxy (PPv1)  → XFF/X-Real-IP == 172.28.0.100
#   3. direct to the PP listener, no header → rejected (mandatory-mode contract)
#   4. direct to the unselected `plain` listener → 200 with XFF 127.0.0.1
#   5. trustedSources: a local trusted peer's valid v1/v2 header is adopted;
#      a 127.0.0.2 peer (outside trusted CIDR 127.0.0.1/32) first proves its
#      effective direct address, then its forged header is not parsed while
#      plain HTTP works. Docker is not used here: Docker Desktop NATs
#      container→host as 127.0.0.1, which cannot distinguish sources.
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
UTILS_DIR="${REPO_ROOT}/edgion-tests/integration/scripts/utils"
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"
# shellcheck source=../utils/isolated_ports.sh
source "${UTILS_DIR}/isolated_ports.sh"
isolated_load_ports proxy_protocol_inbound
isolated_require_ports \
    CTRL_ADMIN_PORT CTRL_GRPC_PORT CTRL_PROBE_PORT GW_ADMIN_PORT \
    GW_PROBE_PORT GW_METRICS_PORT CTRL_METRICS_PORT GW_PP_PORT \
    GW_PLAIN_PORT GW_TRUST_PORT LB_V2_PORT LB_V1_PORT \
    LB_V2_PROBE_PORT LB_V1_PROBE_PORT V2_DUMP_PORT V1_DUMP_PORT \
    TS_HTTP_PORT

BIN_DIR="${REPO_ROOT}/target/debug"
GATEWAY_BIN="${EDGION_GATEWAY_BIN:-$BIN_DIR/edgion-gateway}"
CONTROLLER_BIN="${EDGION_CONTROLLER_BIN:-$BIN_DIR/edgion-controller}"
TEST_SERVER_BIN="${EDGION_TEST_SERVER_BIN:-$BIN_DIR/test_server}"
FIXTURE_DIR="${REPO_ROOT}/edgion-tests/integration/conf/ProxyProtocolInbound"

# One HAProxy serves both legs. Keeping the sender, image, and topology identical
# means the v1 and v2 legs differ in exactly one variable — the `send-proxy` vs
# `send-proxy-v2` flag — so a failure on either is attributable to the protocol
# version and nothing else.
# Private docker network. The client's pinned address is the assertion anchor.
DOCKER_NET="edgion-ppin-test"
DOCKER_SUBNET="172.28.0.0/16"
CLIENT_IP="172.28.0.100"
LB_IP="172.28.0.10"
# A single alpine image covers everything: HAProxy for the load balancer and its
# BusyBox wget for the client probe.
LB_IMAGE="${EDGION_HAPROXY_IMAGE:-haproxy:3.0-alpine}"
LB_CT="edgion-ppin-haproxy"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
log_info()  { echo -e "${BLUE}[INFO]${NC} $1"; }
log_ok()    { echo -e "${GREEN}[OK]${NC}   $1"; }
log_err()   { echo -e "${RED}[ERR]${NC}  $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() { echo ""; echo -e "${BLUE}==== $1 ====${NC}"; }

# =============================================================================
# Prerequisites
# =============================================================================
for bin in "$GATEWAY_BIN" "$CONTROLLER_BIN" "$TEST_SERVER_BIN"; do
    if [ ! -x "$bin" ]; then
        log_err "binary not found: $bin"
        log_info "Build with: cargo build --bin edgion-controller --bin edgion-gateway && cargo build -p edgion-tests --bin test_server"
        exit 1
    fi
done
for tool in docker curl python3 nc lsof; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        log_err "$tool not found (required)"
        exit 1
    fi
done
if ! docker info >/dev/null 2>&1; then
    log_err "docker daemon not reachable"
    exit 1
fi

# =============================================================================
# Shared helpers
# =============================================================================
precheck_port() {
    local port="$1" label="$2"
    if lsof -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
        log_err "port $port ($label) already in use"
        lsof -iTCP:"$port" -sTCP:LISTEN 2>/dev/null | head -3
        return 1
    fi
}

wait_for_tcp_port() {
    local host="$1" port="$2" pid="$3" label="$4" tries="${5:-60}"
    local i
    for i in $(seq 1 "$tries"); do
        if nc -z "$host" "$port" 2>/dev/null; then return 0; fi
        if [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null; then
            log_err "$label exited during startup"; return 1
        fi
        sleep 1
    done
    log_err "$label ${host}:${port} not accepting within ${tries}s"; return 1
}

wait_for_url_200() {
    local url="$1" pid="$2" label="$3" tries="${4:-60}"
    local i code
    for i in $(seq 1 "$tries"); do
        code=$(curl -s -o /dev/null -w "%{http_code}" "$url" 2>/dev/null || true)
        if [ "$code" = "200" ]; then return 0; fi
        if [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null; then
            log_err "$label exited during startup"; return 1
        fi
        sleep 1
    done
    log_err "$label $url did not return 200 within ${tries}s"; return 1
}

# =============================================================================
# State / cleanup
# =============================================================================
WORK_DIR=""; CONF_DIR=""
CTRL_PID=""; GW_PID=""; TS_PID=""
GW_LOG=""; CTRL_LOG=""
TEST_PASSED=0

cleanup() {
    log_section "Cleanup"
    docker rm -f "$LB_CT" >/dev/null 2>&1 || true
    docker network rm "$DOCKER_NET" >/dev/null 2>&1 || true
    for pid in "$GW_PID" "$CTRL_PID" "$TS_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    sleep 1
    for pid in "$GW_PID" "$CTRL_PID" "$TS_PID"; do
        [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null || true
    done
    if [ "$TEST_PASSED" -eq 1 ]; then
        [ -n "$WORK_DIR" ] && rm -rf "$WORK_DIR" || true
        log_info "removed work dir (test passed)"
    elif [ -n "$WORK_DIR" ]; then
        log_warn "keeping work dir for post-mortem: $WORK_DIR"
    fi
}
trap cleanup EXIT

# =============================================================================
# Setup
# =============================================================================
setup() {
    log_section "Setup"
    local work_base="${REPO_ROOT}/tmp/integration_testing"
    mkdir -p "$work_base"
    WORK_DIR=$(mktemp -d "${work_base}/ppin_integration.XXXXXX")
    CONF_DIR="${WORK_DIR}/conf"
    mkdir -p "${WORK_DIR}/logs" "${WORK_DIR}/config" "$CONF_DIR"
    log_info "WORK_DIR=$WORK_DIR"

    # CRD schemas reachable (non-K8s mode loads them from <work_dir>/config/crd).
    ln -s "${REPO_ROOT}/config/crd" "${WORK_DIR}/config/crd"

    # conf_sync mTLS certs under WORK_DIR (server + client share one CA).
    conf_sync_distribute_certs "$WORK_DIR"

    cp "${FIXTURE_DIR}"/*.yaml "${CONF_DIR}/"
    log_ok "seeded $(ls "${CONF_DIR}" | wc -l | tr -d ' ') fixture files"

    cat > "${WORK_DIR}/config/controller.yaml" <<EOF
work_dir: "${WORK_DIR}"

server:
  grpc_listen: "127.0.0.1:${CTRL_GRPC_PORT}"
  admin_listen: "127.0.0.1:${CTRL_ADMIN_PORT}"
  probe_listen: "127.0.0.1:${CTRL_PROBE_PORT}"
  metrics_listen: "127.0.0.1:${CTRL_METRICS_PORT}"

logging:
  log_dir: "${WORK_DIR}/logs"
  log_prefix: edgion-controller
  log_level: info
  json_format: false
  console: false

conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: "${CONF_DIR}"

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
EOF
    conf_sync_security_block server >> "${WORK_DIR}/config/controller.yaml"
    log_ok "wrote controller config"
}

start_test_server() {
    log_section "Start test_server"
    precheck_port "$TS_HTTP_PORT" "test_server http" || return 1
    "$TEST_SERVER_BIN" \
        --http-ports "$TS_HTTP_PORT" \
        --log-level info \
        > "${WORK_DIR}/logs/test_server.log" 2>&1 &
    TS_PID=$!
    disown $TS_PID 2>/dev/null || true
    wait_for_tcp_port 127.0.0.1 "$TS_HTTP_PORT" "$TS_PID" "test_server http" 30 || return 1
    wait_for_url_200 "http://127.0.0.1:${TS_HTTP_PORT}/health" "$TS_PID" "test_server /health" 15 || return 1
    log_ok "test_server up (http ${TS_HTTP_PORT})"
}

start_controller() {
    precheck_port "$CTRL_METRICS_PORT" "controller metrics" || return 1
    log_section "Start controller"
    CTRL_LOG="${WORK_DIR}/logs/controller.log"
    "$CONTROLLER_BIN" \
        -c "${WORK_DIR}/config/controller.yaml" \
        --work-dir "$WORK_DIR" \
        > "$CTRL_LOG" 2>&1 &
    CTRL_PID=$!
    disown $CTRL_PID 2>/dev/null || true
    if ! wait_for_url_200 "http://127.0.0.1:${CTRL_PROBE_PORT}/ready" "$CTRL_PID" "controller /ready" 60; then
        tail -40 "$CTRL_LOG" 2>/dev/null || true
        return 1
    fi
    wait_for_url_200 "http://127.0.0.1:${CTRL_METRICS_PORT}/metrics" \
        "$CTRL_PID" "controller metrics" 30 || return 1
    log_ok "controller ready"
}

start_gateway() {
    log_section "Start gateway"
    GW_LOG="${WORK_DIR}/logs/gateway.log"
    cat > "${WORK_DIR}/gateway.yaml" <<EOF
work_dir: "."
logging:
  log_dir: "${WORK_DIR}/logs"
metrics:
  listen: "127.0.0.1:${GW_METRICS_PORT}"
EOF
    conf_sync_security_block client >> "${WORK_DIR}/gateway.yaml"

    precheck_port "$GW_ADMIN_PORT"   "gateway admin"   || return 1
    precheck_port "$GW_PROBE_PORT"   "gateway probe"   || return 1
    precheck_port "$GW_METRICS_PORT" "gateway metrics" || return 1
    precheck_port "$GW_PP_PORT"      "gateway pp http" || return 1
    precheck_port "$GW_PLAIN_PORT"   "gateway plain http" || return 1
    precheck_port "$GW_TRUST_PORT"   "gateway trusted-sources http" || return 1
    precheck_port "$V2_DUMP_PORT" "v2 byte dumper" || return 1
    precheck_port "$V1_DUMP_PORT" "v1 byte dumper" || return 1

    # Bind on 0.0.0.0 (the gateway always does) so the docker proxies can reach
    # the listeners through host.docker.internal.
    RUST_LOG="info,pingora_proxy=error,pingora_core=error" \
    EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
    "$GATEWAY_BIN" \
        -c "${WORK_DIR}/gateway.yaml" \
        --work-dir "$WORK_DIR" \
        --server-addr "https://127.0.0.1:${CTRL_GRPC_PORT}" \
        --integration-testing-mode \
        --admin-listen "127.0.0.1:${GW_ADMIN_PORT}" \
        --probe-listen "127.0.0.1:${GW_PROBE_PORT}" \
        > "$GW_LOG" 2>&1 &
    GW_PID=$!
    disown $GW_PID 2>/dev/null || true
    wait_for_url_200 "http://127.0.0.1:${GW_PROBE_PORT}/health" "$GW_PID" "gateway /health" 60 || { tail -40 "$GW_LOG"; return 1; }
    wait_for_tcp_port 127.0.0.1 "$GW_PP_PORT"    "$GW_PID" "gateway PP listener"    60 || { tail -40 "$GW_LOG"; return 1; }
    wait_for_tcp_port 127.0.0.1 "$GW_PLAIN_PORT" "$GW_PID" "gateway plain listener" 60 || { tail -40 "$GW_LOG"; return 1; }
    wait_for_tcp_port 127.0.0.1 "$GW_TRUST_PORT" "$GW_PID" "gateway trusted-sources listener" 60 || { tail -40 "$GW_LOG"; return 1; }
    log_ok "gateway up (pp ${GW_PP_PORT}, plain ${GW_PLAIN_PORT}, trusted ${GW_TRUST_PORT})"

    # The flag is only observable at bind time; prove it was actually applied
    # rather than silently defaulted off.
    if grep -q "Inbound PROXY protocol enabled" "$GW_LOG"; then
        log_ok "gateway logged inbound PROXY protocol enablement"
    else
        log_err "gateway never logged inbound PROXY protocol enablement — annotation not applied"
        grep -i "listener" "$GW_LOG" | tail -10 || true
        return 1
    fi
}

# =============================================================================
# Load balancers
# =============================================================================
start_proxies() {
    log_section "Start HAProxy (both PROXY protocol versions)"
    docker rm -f "$LB_CT" >/dev/null 2>&1 || true
    docker network rm "$DOCKER_NET" >/dev/null 2>&1 || true
    docker network create --subnet "$DOCKER_SUBNET" "$DOCKER_NET" >/dev/null

    # HAProxy is the reference implementation of this protocol (the spec is a
    # HAProxy document) and emits both versions from one binary: `send-proxy-v2`
    # writes the 12-byte binary signature + PROXY command + address block,
    # `send-proxy` writes the v1 text line. nginx is deliberately not used — it
    # can only ever send v1, so it could not cover the v2 leg, and mixing senders
    # would make the two legs differ by more than the protocol version.
    cat > "${WORK_DIR}/haproxy.cfg" <<EOF
global
    daemon
defaults
    mode tcp
    timeout connect 5s
    timeout client 30s
    timeout server 30s
listen ppv2
    bind *:${LB_V2_PORT}
    server edgion host.docker.internal:${GW_PP_PORT} send-proxy-v2
listen ppv1
    bind *:${LB_V1_PORT}
    server edgion host.docker.internal:${GW_PP_PORT} send-proxy
listen ppv2probe
    bind *:${LB_V2_PROBE_PORT}
    server dump host.docker.internal:${V2_DUMP_PORT} send-proxy-v2
listen ppv1probe
    bind *:${LB_V1_PROBE_PORT}
    server dump host.docker.internal:${V1_DUMP_PORT} send-proxy
EOF

    docker run -d --name "$LB_CT" \
        --network "$DOCKER_NET" --ip "$LB_IP" \
        --add-host host.docker.internal:host-gateway \
        -v "${WORK_DIR}/haproxy.cfg:/usr/local/etc/haproxy/haproxy.cfg:ro" \
        "$LB_IMAGE" >/dev/null

    sleep 2
    if [ "$(docker inspect -f '{{.State.Running}}' "$LB_CT" 2>/dev/null)" != "true" ]; then
        log_err "$LB_CT failed to start"
        docker logs "$LB_CT" 2>&1 | tail -20 || true
        return 1
    fi
    log_ok "haproxy ${LB_IP}: v2 on ${LB_V2_PORT}, v1 on ${LB_V1_PORT}"
}

# Probe from a container pinned to CLIENT_IP; prints the response body.
# BusyBox wget keeps this to one line and needs no extra image.
probe_via() {
    local target="$1"
    docker run --rm --network "$DOCKER_NET" --ip "$CLIENT_IP" \
        "$LB_IMAGE" \
        wget -q -T 8 -O - "http://${target}/headers" 2>/dev/null || true
}

# Extract one header value out of the /headers JSON echo.
header_value() {
    local body="$1" name="$2"
    printf '%s' "$body" | python3 -c "
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
print(data.get('headers', {}).get(sys.argv[1], ''))
" "$name"
}

# =============================================================================
# Phases
# =============================================================================
FAILURES=0
fail() { log_err "$1"; FAILURES=$((FAILURES + 1)); }

# Capture the first bytes a proxy actually puts on the wire.
#
# Without this the two legs would assert the same thing (the client IP), so a
# proxy silently emitting the other version would still pass and we would
# wrongly believe both parser paths were covered. The probe listener in each
# proxy's config is a copy of its real one, so this observes the real emission.
capture_wire_bytes() {
    local dump_port="$1" probe_target="$2" out="$3"
    python3 - "$dump_port" "$out" <<'PY' &
import socket, sys
port, out = int(sys.argv[1]), sys.argv[2]
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("0.0.0.0", port)); srv.listen(1); srv.settimeout(20)
try:
    conn, _ = srv.accept()
    conn.settimeout(5)
    open(out, "w").write(conn.recv(64).hex())
    conn.close()
except Exception as exc:
    open(out, "w").write("ERROR:%s" % exc)
finally:
    srv.close()
PY
    local dumper_pid=$!
    sleep 1
    # This probe checks only the emitted protocol version. Reuse the running
    # load balancer so container startup cannot consume the capture deadline.
    # Address adoption is tested separately with the pinned client container.
    docker exec "$LB_CT" \
        wget -q -T 5 -O - "http://${probe_target}/" >/dev/null 2>&1 || true
    wait "$dumper_pid" 2>/dev/null || true
}

phase_wire_versions() {
    log_section "Phase: PROXY protocol version each proxy puts on the wire"
    local v2_sig="0d0a0d0a000d0a515549540a"   # the 12-byte v2 signature
    local hex=""

    capture_wire_bytes "$V2_DUMP_PORT" "${LB_IP}:${LB_V2_PROBE_PORT}" "${WORK_DIR}/v2_wire.hex"
    hex=$(cat "${WORK_DIR}/v2_wire.hex" 2>/dev/null || true)
    log_info "v2 leg first bytes: ${hex:0:32}"
    case "$hex" in
        "${v2_sig}21"*) log_ok "v2 leg is genuinely PROXY protocol v2 (signature + 0x21 PROXY command)" ;;
        "${v2_sig}"*)   fail "v2 leg sent the v2 signature but not a PROXY command: ${hex:0:32}" ;;
        50524f5859*)   fail "the v2 leg sent v1 ('PROXY ' in ASCII) — it is not testing v2" ;;
        *)             fail "v2 leg sent no recognisable PROXY header: '${hex:0:32}'" ;;
    esac

    capture_wire_bytes "$V1_DUMP_PORT" "${LB_IP}:${LB_V1_PROBE_PORT}" "${WORK_DIR}/v1_wire.hex"
    hex=$(cat "${WORK_DIR}/v1_wire.hex" 2>/dev/null || true)
    log_info "v1 leg first bytes: ${hex:0:32}"
    case "$hex" in
        50524f585920*) log_ok "v1 leg is genuinely PROXY protocol v1 ('PROXY ' text form)" ;;
        "${v2_sig}"*)  fail "the v1 leg sent v2 — send-proxy and send-proxy-v2 are swapped" ;;
        *)             fail "v1 leg sent no recognisable PROXY header: '${hex:0:32}'" ;;
    esac
}

phase_proxied() {
    local label="$1" target="$2"
    log_section "Phase: ${label}"
    local body xff xri
    body=$(probe_via "$target")
    if [ -z "$body" ]; then
        fail "${label}: no response body through the proxy"
        docker logs "$LB_CT" 2>&1 | tail -10 || true
        tail -20 "$GW_LOG" || true
        return
    fi
    xff=$(header_value "$body" x-forwarded-for)
    xri=$(header_value "$body" x-real-ip)
    log_info "${label}: x-forwarded-for='${xff}' x-real-ip='${xri}'"

    case "$xff" in
        *"$CLIENT_IP"*) log_ok "${label}: X-Forwarded-For carries the real client ${CLIENT_IP}" ;;
        *) fail "${label}: X-Forwarded-For '${xff}' does not contain ${CLIENT_IP} — the PROXY header was not honoured" ;;
    esac
    case "$xri" in
        *"$CLIENT_IP"*) log_ok "${label}: X-Real-IP carries the real client ${CLIENT_IP}" ;;
        *) fail "${label}: X-Real-IP '${xri}' does not contain ${CLIENT_IP}" ;;
    esac
}

phase_missing_header() {
    log_section "Phase: no header on a PROXY-protocol listener"
    # Mandatory mode: the gateway must drop this connection rather than serve it.
    # Keep the `-w` output and the exit status separate — folding a fallback into
    # the command substitution would concatenate both and never match.
    local code="" exit_code=0
    code=$(curl -s -o /dev/null -m 5 -w "%{http_code}" "http://127.0.0.1:${GW_PP_PORT}/headers" 2>/dev/null) || exit_code=$?
    if [ "$code" = "200" ]; then
        fail "the request was served (HTTP 200) — mandatory-mode contract broken"
        return
    fi
    if [ "$exit_code" -eq 0 ] && [ "$code" != "000" ]; then
        fail "expected the connection to be dropped, got HTTP ${code}"
        return
    fi
    log_ok "connection without a PROXY header got no HTTP response (curl exit ${exit_code}, code '${code}')"

    # Positive confirmation: the drop must come from the PROXY protocol parser,
    # not from an unrelated listener failure.
    if grep -qi "proxy protocol" "$GW_LOG"; then
        log_ok "gateway logged a proxy protocol handshake rejection"
    else
        fail "gateway log has no proxy protocol rejection — the connection failed for some other reason"
        tail -20 "$GW_LOG" || true
    fi
}

phase_unselected_listener() {
    log_section "Phase: unselected listener remains disabled"
    # The strict annotation selects only `pp`, so a plain request must be served normally.
    local body code xff
    code=$(curl -s -o "${WORK_DIR}/plain_body.json" -m 5 -w "%{http_code}" \
        "http://127.0.0.1:${GW_PLAIN_PORT}/headers" 2>/dev/null || echo "000")
    if [ "$code" != "200" ]; then
        fail "plain listener returned ${code}, expected 200 — listener selection not honoured"
        return
    fi
    body=$(cat "${WORK_DIR}/plain_body.json")
    xff=$(header_value "$body" x-forwarded-for)
    log_info "plain listener: x-forwarded-for='${xff}'"
    case "$xff" in
        *127.0.0.1*) log_ok "plain listener served normally with the direct peer address" ;;
        *) fail "plain listener X-Forwarded-For '${xff}' does not contain 127.0.0.1" ;;
    esac
}

UNTRUSTED_SOURCE_IP="127.0.0.2"

# Bind an explicit source address so trusted (127.0.0.1) and untrusted
# (127.0.0.2) probes are distinct on every platform, including Docker Desktop
# where container→host.docker.internal is NATed to 127.0.0.1.
source_probe() {
    local source_ip="$1" version="${2:-}"
    python3 - "$GW_TRUST_PORT" "$source_ip" "$version" <<'PY'
import socket, struct, sys
port, source_ip, version = int(sys.argv[1]), sys.argv[2], sys.argv[3]
request = b"GET /headers HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
if version == "v1":
    prefix = f"PROXY TCP4 203.0.113.77 127.0.0.1 45678 {port}\r\n".encode()
elif version == "v2":
    addresses = socket.inet_aton("203.0.113.77") + socket.inet_aton("127.0.0.1") + struct.pack("!HH", 45678, port)
    prefix = b"\r\n\r\n\x00\r\nQUIT\n" + bytes((0x21, 0x11)) + struct.pack("!H", len(addresses)) + addresses
elif version == "":
    prefix = b""
else:
    raise SystemExit(f"unsupported PROXY probe version {version}")
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.settimeout(5)
try:
    sock.bind((source_ip, 0))
    sock.connect(("127.0.0.1", port))
except OSError as exc:
    print(f"ERROR: cannot establish probe from explicit source {source_ip}: {exc}", file=sys.stderr)
    sock.close()
    raise SystemExit(2)
sock.sendall(prefix + request)
chunks = []
while True:
    chunk = sock.recv(65536)
    if not chunk:
        break
    chunks.append(chunk)
sock.close()
print(b"".join(chunks).decode("latin1", "replace"))
PY
}

http_body() {
    printf '%s' "$1" | python3 -c "
import sys
data = sys.stdin.buffer.read()
print(data.split(b'\r\n\r\n', 1)[-1].decode('utf-8', 'replace'))
"
}

trusted_probe() {
    local response
    response=$(source_probe "127.0.0.1" "$1") || return 1
    http_body "$response"
}

phase_trusted_sources() {
    log_section "Phase: trustedSources adoption and forged-header rejection"
    local version body xff plain forged forged_rc status_line
    for version in v1 v2; do
        body=$(trusted_probe "$version")
        xff=$(header_value "$body" x-forwarded-for)
        case "$xff" in
            *203.0.113.77*) log_ok "trusted local ${version} header was adopted" ;;
            *) fail "trusted local ${version} header was not adopted: '${xff}'" ;;
        esac
    done

    if ! plain=$(source_probe "$UNTRUSTED_SOURCE_IP"); then
        fail "trustedSources topology cannot prove a source outside 127.0.0.1/32"
        return
    fi
    body=$(http_body "$plain")
    xff=$(header_value "$body" x-forwarded-for)
    case "$xff" in
        "") fail "untrusted peer response did not expose an effective direct address"; return ;;
        *"$UNTRUSTED_SOURCE_IP"*)
            log_ok "untrusted peer effective direct address '${xff}' is outside trusted CIDR 127.0.0.1/32"
            ;;
        *127.0.0.1*)
            fail "untrusted peer resolved inside trusted CIDR 127.0.0.1/32; source distinction is not valid"
            return
            ;;
        *203.0.113.77*) fail "untrusted plain peer unexpectedly acquired the forged identity"; return ;;
        *) fail "untrusted peer X-Forwarded-For '${xff}' is not ${UNTRUSTED_SOURCE_IP}"; return ;;
    esac

    for version in v1 v2; do
        set +e
        forged=$(source_probe "$UNTRUSTED_SOURCE_IP" "$version")
        forged_rc=$?
        set -e
        if [ "$forged_rc" -ne 0 ] || [ -z "$forged" ]; then
            fail "untrusted forged ${version} probe did not receive a response from the live Gateway (rc=${forged_rc})"
            continue
        fi
        status_line=$(printf '%s' "$forged" | sed -n '1{s/\r$//;p;}')
        case "$status_line" in
            "HTTP/1.0 200"*|"HTTP/1.1 200"*)
                fail "untrusted forged ${version} request reached the /headers backend"
                ;;
            "HTTP/1.0 "[45][0-9][0-9]*|"HTTP/1.1 "[45][0-9][0-9]*)
                case "$forged" in
                    *203.0.113.77*) fail "untrusted forged ${version} identity appeared in the response" ;;
                    *) log_ok "untrusted forged ${version} bytes reached the Gateway, were handled as ordinary invalid HTTP (${status_line}), and did not reach the 200 /headers backend" ;;
                esac
                ;;
            *) fail "untrusted forged ${version} probe returned unexpected status '${status_line}'" ;;
        esac
        if ! kill -0 "$GW_PID" 2>/dev/null; then
            fail "Gateway exited after the untrusted forged ${version} probe"
        fi
    done
}

# =============================================================================
# Main
# =============================================================================
main() {
    log_section "Inbound PROXY protocol integration test"
    setup
    start_test_server
    start_controller
    start_gateway
    start_proxies

    phase_wire_versions
    phase_proxied "HAProxy PPv2" "${LB_IP}:${LB_V2_PORT}"
    phase_proxied "HAProxy PPv1" "${LB_IP}:${LB_V1_PORT}"
    phase_missing_header
    phase_unselected_listener
    phase_trusted_sources

    log_section "Result"
    if [ "$FAILURES" -eq 0 ]; then
        TEST_PASSED=1
        log_ok "all phases passed"
        return 0
    fi
    log_err "${FAILURES} assertion(s) failed"
    return 1
}

main "$@"
