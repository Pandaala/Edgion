#!/usr/bin/env bash
# =============================================================================
# Two-hop gateway chain — does a PROXY-derived client address survive the next
# hop?
#
# Contract: docs/en/dev-guide/annotations-guide.md
#
# WHY THIS EXISTS: the single-hop suite proves a PROXY header is parsed. It says
# nothing about whether the address then reaches a SECOND gateway, which is the
# deployment this feature was actually asked for.
#
#   client 172.29.0.100 → haproxy (send-proxy-v2) → gw1 :58530 → gw2 :58531
#                                                                    ↓
#                                                     test_server :30062 /headers
#
# gw1 gets the client from the PROXY header and appends it to X-Forwarded-For.
# gw2 has no inbound PROXY protocol annotation — nothing sends it a header — and resolves
# the client by walking XFF, which only works because the GatewayClass carries a
# parametersRef to an EdgionGatewayConfig listing hop 1 in realIp.trustedIps.
# The config also uses X-Edgion-Remote-IP to prove the upstream RemoteIp header
# is independently configurable from the downstream realIp input header.
#
# THE LOAD-BEARING ASSERTION IS X-EDGION-REMOTE-IP, NOT X-Forwarded-For. XFF is appended
# unconditionally by every hop, so it carries the client whether or not hop 2
# resolved anything; asserting on it alone would pass while every IP-based plugin
# at hop 2 still saw hop 1. The configured RemoteIp header is written from the
# RESOLVED address, so it
# is the only field that fails when the chain is misconfigured. Confirmed by
# deleting realIp from EdgionGatewayConfig__chain-egc.yaml: X-Edgion-Remote-IP degrades to
# 127.0.0.1 and this suite fails, while both XFF assertions still pass.
#
# Only the v2 leg is driven. What this suite tests is address propagation across
# hops, not the parsers — the single-hop suite covers v1 and v2 both.
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
UTILS_DIR="${REPO_ROOT}/edgion-tests/integration/scripts/utils"
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

BIN_DIR="${REPO_ROOT}/target/debug"
GATEWAY_BIN="${EDGION_GATEWAY_BIN:-$BIN_DIR/edgion-gateway}"
CONTROLLER_BIN="${EDGION_CONTROLLER_BIN:-$BIN_DIR/edgion-controller}"
TEST_SERVER_BIN="${EDGION_TEST_SERVER_BIN:-$BIN_DIR/test_server}"
FIXTURE_DIR="${REPO_ROOT}/edgion-tests/integration/conf/ProxyProtocolChain"

# Dedicated port block (585xx: clear of the 12xxx shared stack, the 583xx
# plugin-policy block, and the 584xx single-hop PROXY protocol block).
CTRL_ADMIN_PORT=58520
CTRL_GRPC_PORT=58521
CTRL_PROBE_PORT=58522
GW_ADMIN_PORT=58523
GW_PROBE_PORT=58524
GW_METRICS_PORT=58525
GW_PP_PORT=58530      # hop 1: PROXY protocol required
GW_HOP2_PORT=58531    # hop 2: plain HTTP, reached from hop 1
# HAProxy fronts hop 1, speaking PROXY protocol v2.
LB_V2_PORT=58540
# Wire-version preflight: each leg gets a twin listener configured identically
# but pointed at a byte dumper on the host, so what we observe is genuinely what
# the real leg emits.
LB_V2_PROBE_PORT=58542
V2_DUMP_PORT=58544
# Backend port: stays in the 3xxxx test_server range by convention, but off
# the shared stack's numbers so the two can run in any order.
TS_HTTP_PORT=30062

# Private docker network. The client's pinned address is the assertion anchor.
DOCKER_NET="edgion-ppchain-test"
DOCKER_SUBNET="172.29.0.0/16"  # distinct from the inbound suite: docker refuses overlapping pools
CLIENT_IP="172.29.0.100"
LB_IP="172.29.0.10"
# A single alpine image covers everything: HAProxy for the load balancer and its
# BusyBox wget for the client probe.
LB_IMAGE="${EDGION_HAPROXY_IMAGE:-haproxy:3.0-alpine}"
LB_CT="edgion-ppchain-haproxy"

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
    WORK_DIR=$(mktemp -d "${work_base}/ppchain_integration.XXXXXX")
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

logging:
  log_dir: "${WORK_DIR}/logs"
  log_prefix: edgion-controller
  log_level: info
  json_format: false
  console: false

conf_center:
  type: file_system
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
    precheck_port "$GW_HOP2_PORT"    "gateway hop2 http" || return 1
    precheck_port "$V2_DUMP_PORT" "byte dumper" || return 1

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
    wait_for_tcp_port 127.0.0.1 "$GW_HOP2_PORT" "$GW_PID" "gateway hop2 listener" 60 || { tail -40 "$GW_LOG"; return 1; }
    log_ok "gateway up (hop1 ${GW_PP_PORT}, hop2 ${GW_HOP2_PORT})"

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
listen ppv2probe
    bind *:${LB_V2_PROBE_PORT}
    server dump host.docker.internal:${V2_DUMP_PORT} send-proxy-v2
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
    log_ok "haproxy ${LB_IP}: v2 on ${LB_V2_PORT}"
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
    # One connection through the proxy is enough; the payload is irrelevant.
    docker run --rm --network "$DOCKER_NET" --ip "$CLIENT_IP" "$LB_IMAGE" \
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
}

phase_chain() {
    log_section "Phase: two-hop chain"
    local body xff remote_ip_header default_remote_ip_header
    body=$(probe_via "${LB_IP}:${LB_V2_PORT}")
    if [ -z "$body" ]; then
        fail "no response through the chain"
        docker logs "$LB_CT" 2>&1 | tail -10 || true
        tail -30 "$GW_LOG" || true
        return
    fi
    xff=$(header_value "$body" x-forwarded-for)
    remote_ip_header=$(header_value "$body" x-edgion-remote-ip)
    default_remote_ip_header=$(header_value "$body" x-real-ip)
    log_info "chain x-forwarded-for='${xff}' x-edgion-remote-ip='${remote_ip_header}' x-real-ip='${default_remote_ip_header}'"

    # The load-bearing assertion. The configured RemoteIp header is written from
    # the RESOLVED address,
    # so it is the only field that notices when hop 2 fails to walk the chain.
    if [ "$remote_ip_header" = "$CLIENT_IP" ]; then
        log_ok "hop 2 resolved the real client ${CLIENT_IP}"
    else
        fail "hop 2 X-Edgion-Remote-IP is '${remote_ip_header}', expected ${CLIENT_IP} — the chain lost the client"
    fi
    if [ -n "$default_remote_ip_header" ]; then
        fail "custom RemoteIp header must replace the default X-Real-IP header, got '${default_remote_ip_header}'"
    else
        log_ok "custom RemoteIp header replaced default X-Real-IP"
    fi

    case "$xff" in
        *"$CLIENT_IP"*) log_ok "X-Forwarded-For carries the client" ;;
        *) fail "X-Forwarded-For '${xff}' lost the client" ;;
    esac

    # Two entries: the client (appended by hop 1) and hop 1's own address
    # (appended by hop 2). A single entry means hop 1 never reached hop 2.
    case "$xff" in
        *,*) log_ok "X-Forwarded-For shows both hops: ${xff}" ;;
        *) fail "X-Forwarded-For '${xff}' has one entry — hop 1 did not extend the chain" ;;
    esac
}

# =============================================================================
# Main
# =============================================================================
main() {
    log_section "Two-hop gateway chain integration test"
    setup
    start_test_server
    start_controller
    start_gateway
    start_proxies

    phase_wire_versions
    phase_chain

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
