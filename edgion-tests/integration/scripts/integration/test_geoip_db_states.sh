#!/usr/bin/env bash
# Standalone preflight for the GeoIP database-state matrix that the shared
# City-tier integration topology structurally cannot reach.
#
# `geoip` is process-wide, restart-only config (edgion-gateway/src/geoip/config.rs):
# one mmap, one updater task, one process. "no database", "Country-tier
# database", "City-tier database", and "updater hot-swap" are four mutually
# exclusive configurations of that one process, so they cannot coexist with the
# shared sweep's single City-tier gateway (see EdgionPlugins/GeoRestriction,
# TCPRoute/GeoRestriction, TLSRoute/GeoRestriction, which cover the reachable
# City-tier decision table against that shared gateway).
#
# It runs an ISOLATED stack (its own minimal Controller + Gateway on dedicated
# ports) so it does not disturb the main sweep's single shared gateway: geoip is
# process-global and restart-only, so "no database", "Country-tier" and
# a Country-tier database cannot coexist with the shared City-tier gateway.
#
# Boot model mirrors test_fips_policy.sh / test_gateway_admin_auth.sh: one
# minimal file_system Controller is the gRPC sync backend and stays up for the
# whole script; the Gateway is relaunched once per phase against it (only the
# `gateway.geoip` block changes between launches — every K8s-style resource is
# seeded once, up front, and never changes). Two tiny local HTTP/TCP backends
# and one local "database mirror" HTTP server stand in for the shared stack's
# `test_server`, so this preflight has no dependency on it.
#
# Phase -> assertion summary (see the task brief for the full row list):
#   1. No database (db_path configured but missing): gateway starts and serves;
#      edgion_geoip_db_loaded == 0; an HTTP GeoIpLocation deny rule with
#      failOpen:false rejects (fail-closed); failOpen:true passes; an L4
#      GeoIpLocation deny rule with failOpen:false denies and bumps
#      edgion_geoip_denied_total{reason="db_unavailable"}; failOpen:true
#      admits. A CIDR-only rule (`key: cidr`) on BOTH layers is immune to all
#      of this: it needs no GeoIP database at all, so it must behave
#      identically whether or not one is loaded (the bug
#      `GeoIpLocationConfig::source_needs_geo_facts` exists to prevent — see
#      geoiplocation-plugin-report.md's "Correctness fix" section).
#   2. Country-tier: country rules work; a city rule's predicate is simply
#      absent (no "database tier" concept -- onMissing governs, same as any
#      other missing value) -- proved by the city-rule-inert check itself, not
#      by an admin-endpoint snapshot (there is no admin endpoint for this).
#   3. City-tier: geo.remote.coord is present in the
#      access log and masked ("***"); the city rule (inert against Country-tier
#      in Phase 2) becomes live, proving a City-tier database is actually loaded.
#   4. Updater hot-swap: db_path starts at the Country fixture, update_url
#      points at a local file server serving the City fixture. A city rule that
#      was inert (Country-tier, pre-swap) becomes live (City-tier, post-swap) --
#      the same behavioral proof Phase 3 uses, polled until it flips; <db_path>.meta
#      is written; no <db_path>.tmp remains.
#
# Known, deliberately-not-hidden gap: both fixtures share one pinned
# build_epoch constant (edgion-tests/geoip/generate.py monkeypatches
# `time.time` once, before writing EITHER file — see generate.py:52-55 and
# README.md's "Reproducibility" section), so build_epoch does NOT flip across
# the Phase 4 swap. This script reports both values and does not assert a
# change — asserting one would be asserting something the fixtures cannot
# produce, not verifying the product.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
UTILS_DIR="${REPO_ROOT}/edgion-tests/integration/scripts/utils"
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

GATEWAY_BIN="${EDGION_GATEWAY_BIN:-$REPO_ROOT/target/debug/edgion-gateway}"
CONTROLLER_BIN="${EDGION_CONTROLLER_BIN:-$REPO_ROOT/target/debug/edgion-controller}"

COUNTRY_FIXTURE="${REPO_ROOT}/edgion-tests/geoip/edgion-test-country.mmdb"
CITY_FIXTURE="${REPO_ROOT}/edgion-tests/geoip/edgion-test-city.mmdb"

# A token >= 32 chars so the gateway does not also emit the weak-token WARN.
TEST_TOKEN="integration-geoip-db-states-admin-token-0123456789"

# Dedicated port block, clear of every other preflight's range (verified by
# grepping every 58xxx literal across this directory before picking this
# block): controller-noauth 58200-58213, gateway-admin-auth 58300-58307,
# fips 58310-58317, plugin-policy 58320-58331, proxy-protocol 58420-58445,
# proxy-protocol-chain 58520-58544/58592.
CTRL_ADMIN_PORT=58600
CTRL_GRPC_PORT=58601
CTRL_PROBE_PORT=58602
GW_ADMIN_PORT=58603
GW_PROBE_PORT=58604
GW_METRICS_PORT=58605
GW_HTTP_PORT=58606
GW_TCP_DENY_PORT=58607
GW_TCP_ALLOW_PORT=58608
TCP_BACKEND_PORT=58609
UPDATE_SERVER_PORT=58610
HTTP_BACKEND_PORT=58611
GW_TCP_CIDR_PORT=58612
CTRL_METRICS_PORT=58613

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
if [ ! -f "$COUNTRY_FIXTURE" ] || [ ! -f "$CITY_FIXTURE" ]; then
    echo "ERROR: geoip test fixtures not found under ${REPO_ROOT}/edgion-tests/geoip"
    exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 not found (required for the local HTTP/TCP test backends)"
    exit 1
fi

# =============================================================================
# Shared helpers
# =============================================================================

# Keep every isolated process directory on the repository's filesystem. A bare
# `mktemp -d` uses the host's system TMPDIR (for example `/private/var/folders`
# on macOS), which may have different free space from the integration workspace.
# That is correctness-significant here: the LocalFile access-log sink sheds at
# its disk-usage threshold, while Phases 2 and 3 assert on the resulting file.
TEST_WORK_ROOT=""

init_test_work_root() {
    local work_base="${REPO_ROOT}/tmp/integration_testing"
    mkdir -p "$work_base"
    TEST_WORK_ROOT=$(mktemp -d "${work_base}/geoip_db_states.XXXXXX")
    echo "INFO: GeoIP database-state work directory: ${TEST_WORK_ROOT}"
}

new_test_dir() {
    local label="$1"
    mktemp -d "${TEST_WORK_ROOT}/${label}.XXXXXX"
}

# The gateway's LocalFile access-log sink sheds to unhealthy — and then silently
# drops every access-log entry — once the log volume reaches
# `disk_usage_threshold_percent` (product default 90, see
# `default_disk_usage_threshold_percent` in edgion-resources/src/output.rs).
# Phases 2 and 3 assert on access-log content (the geo block's status/country
# fields, masked geo.remote.coord), so on a >=90%-full host those assertions fail
# for a reason
# unrelated to geoip. Detect the unfit environment up front (same precedent as
# test_plugin_policy.sh's precheck_disk_space) rather than letting it surface
# later as a confusing content-assertion failure. We do NOT override the
# product threshold — the disk check is correct and must keep its production
# default.
readonly ACCESS_LOG_DISK_THRESHOLD_PERCENT=90

precheck_disk_space() {
    local dir="$1"
    # Replicate the gateway's own disk_usage_percent() formula rather than trust
    # df's Capacity column — see test_plugin_policy.sh's identical helper for
    # the full rationale on the denominator mismatch.
    local pct
    pct=$(df -P "$dir" 2>/dev/null | awk 'NR==2 && $2+0>0 { printf "%d", (($2-$4)*100)/$2 }')
    if [ -z "$pct" ]; then
        echo "WARN: could not determine disk usage for $dir; skipping disk precheck"
        return 0
    fi
    if [ "$pct" -ge "$ACCESS_LOG_DISK_THRESHOLD_PERCENT" ]; then
        echo "FAIL: disk for $dir computes to ${pct}% full by the gateway's own formula (sheds at >= ${ACCESS_LOG_DISK_THRESHOLD_PERCENT}%)"
        echo "FAIL: the gateway would silently drop access-log entries, so Phase 2/3's log assertions cannot pass"
        echo "FAIL: free enough space to drop BELOW ${ACCESS_LOG_DISK_THRESHOLD_PERCENT}% and re-run — this is an environment issue, not a geoip failure"
        return 1
    fi
}

precheck_port() {
    local port="$1" label="$2"
    if lsof -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
        echo "FAIL: port $port ($label) already in use; stop the conflicting process or rerun later"
        lsof -iTCP:"$port" -sTCP:LISTEN 2>/dev/null | head -3
        return 1
    fi
}

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

http_status() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

# Poll an HTTP endpoint (with a Host header) until it returns the wanted
# status code. Used in place of an admin-endpoint snapshot (there is none) to
# prove a config-driven state change actually took effect -- e.g. Phase 4's
# hot-swap, where the city rule flips from inert (200) to live (403) once the
# updater's swap completes.
wait_for_http_status() {
    local url="$1" host_header="$2" want="$3" tries="${4:-30}"
    local i got
    for i in $(seq 1 "$tries"); do
        got=$(http_status -H "Host: ${host_header}" "$url")
        if [ "$got" = "$want" ]; then
            return 0
        fi
        sleep 1
    done
    echo "FAIL: $url (Host: $host_header) never returned $want (last saw '$got')"
    return 1
}

# The LocalFile access-log sink writes asynchronously (a background flush
# task, not inline with the request), so a grep run immediately after a curl
# returns can race the write. Poll briefly instead of a one-shot grep.
wait_for_access_log_contains() {
    local needle="$1" tries="${2:-15}"
    local i
    for i in $(seq 1 "$tries"); do
        if grep -q -- "$needle" "$ACCESS_LOG" 2>/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

# Sum of every Prometheus line whose metric name is exactly `$1` (bare or with
# any label set — this build always adds a `service="edgion-gateway"` label,
# so an exact "name{labels}" match is too brittle) AND whose label set
# contains every remaining argument as a literal substring (e.g.
# 'layer="tcp"' 'reason="db_unavailable"'). Prints 0 when the series has not
# been emitted yet — counters here follow the "absent until first record"
# cold pattern (registry.rs's own comment on record_geoip_denied and friends).
metric_value() {
    local name="$1"; shift
    local raw
    raw=$(curl -s "http://127.0.0.1:${GW_METRICS_PORT}/metrics" | grep -E "^${name}(\{|[[:space:]])" || true)
    local pat
    for pat in "$@"; do
        raw=$(printf '%s\n' "$raw" | grep -F -- "$pat" || true)
    done
    printf '%s\n' "$raw" | awk '{sum+=$NF} END{print (NR==0 ? 0 : sum)}'
}

# TCP probe: connect (optionally source-bound), send bytes, read back.
# Prints "ECHO" if bytes came back, "CLOSED" if the peer closed with no data,
# "ERR:..." for anything else — never conflated with a silent pass/fail.
tcp_probe() {
    local port="$1" src="${2:-}"
    python3 - "$port" "$src" <<'PY'
import socket, sys
port = int(sys.argv[1])
src = sys.argv[2] if len(sys.argv) > 2 else ""
try:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    if src:
        s.bind((src, 0))
    s.settimeout(5)
    s.connect(("127.0.0.1", port))
    s.sendall(b"probe")
    data = s.recv(64)
    s.close()
    print("ECHO" if data else "CLOSED")
except (ConnectionResetError, BrokenPipeError, socket.timeout):
    print("CLOSED")
except Exception as e:
    print("ERR:%s" % e)
PY
}

PASS_COUNT=0
FAIL_COUNT=0
check() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "PASS: $label"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        echo "FAIL: $label (got '$got', want '$want')"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
}
check_true() {
    local label="$1" cond="$2"
    if [ "$cond" = "1" ]; then
        echo "PASS: $label"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        echo "FAIL: $label"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
}

# =============================================================================
# Local test backends (HTTP echo-200, TCP echo, "database mirror" file server)
# =============================================================================
SCRATCH_DIR=""
HTTP_BACKEND_PID=""
TCP_BACKEND_PID=""
UPDATE_SERVER_PID=""

start_backends() {
    SCRATCH_DIR=$(new_test_dir "backends")

    cat > "${SCRATCH_DIR}/http_backend.py" <<'PY'
import http.server, socketserver, sys

port = int(sys.argv[1])

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(b"OK")
    def log_message(self, fmt, *args):
        pass

with socketserver.TCPServer(("127.0.0.1", port), Handler) as httpd:
    httpd.allow_reuse_address = True
    httpd.serve_forever()
PY

    cat > "${SCRATCH_DIR}/tcp_backend.py" <<'PY'
import socket, sys, threading

port = int(sys.argv[1])

def handle(conn):
    with conn:
        while True:
            try:
                data = conn.recv(4096)
            except OSError:
                break
            if not data:
                break
            try:
                conn.sendall(data)
            except OSError:
                break

s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", port))
s.listen(5)
while True:
    conn, _ = s.accept()
    threading.Thread(target=handle, args=(conn,), daemon=True).start()
PY

    # "Database mirror": serves edgion-test-city.mmdb with a fixed delay before
    # the first response byte. The delay is not cosmetic: it makes Phase 4's
    # pre-swap behavior assertion deterministic instead
    # of racing the updater's own 0-5s startup jitter (updater.rs
    # sleep_with_jitter) against however long our own curl takes to run.
    cat > "${SCRATCH_DIR}/update_server.py" <<'PY'
import http.server, socketserver, sys, time

port = int(sys.argv[1])
directory = sys.argv[2]
delay = float(sys.argv[3])

class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=directory, **kwargs)
    def do_GET(self):
        if delay:
            time.sleep(delay)
        super().do_GET()
    def log_message(self, fmt, *args):
        pass

with socketserver.TCPServer(("0.0.0.0", port), Handler) as httpd:
    httpd.allow_reuse_address = True
    httpd.serve_forever()
PY

    mkdir -p "${SCRATCH_DIR}/update_dir"
    cp "$CITY_FIXTURE" "${SCRATCH_DIR}/update_dir/edgion-test-city.mmdb"
    # `http.server`'s SimpleHTTPRequestHandler answers conditional GETs
    # (If-Modified-Since) off the served file's own mtime. `UpdateState::load`
    # seeds the updater's first `If-Modified-Since` from db_path's mtime when
    # no `.meta` sidecar exists yet (updater.rs), and Phase 4 copies the
    # Country fixture into db_path AFTER this file is written here — a later
    # mtime on db_path than on this mirrored file would make the very first
    # conditional GET a false 304, and the swap would never happen. Pin this
    # file's mtime far in the future so it always reads as "newer",
    # independent of copy ordering or wall-clock skew between the two copies.
    touch -t 209912312359 "${SCRATCH_DIR}/update_dir/edgion-test-city.mmdb"

    precheck_port "$HTTP_BACKEND_PORT"   "local HTTP backend"   || return 1
    precheck_port "$TCP_BACKEND_PORT"    "local TCP backend"    || return 1
    precheck_port "$UPDATE_SERVER_PORT"  "local update server"  || return 1

    python3 "${SCRATCH_DIR}/http_backend.py" "$HTTP_BACKEND_PORT" \
        > "${SCRATCH_DIR}/http_backend.log" 2>&1 &
    HTTP_BACKEND_PID=$!
    python3 "${SCRATCH_DIR}/tcp_backend.py" "$TCP_BACKEND_PORT" \
        > "${SCRATCH_DIR}/tcp_backend.log" 2>&1 &
    TCP_BACKEND_PID=$!
    python3 "${SCRATCH_DIR}/update_server.py" "$UPDATE_SERVER_PORT" "${SCRATCH_DIR}/update_dir" 3 \
        > "${SCRATCH_DIR}/update_server.log" 2>&1 &
    UPDATE_SERVER_PID=$!

    wait_for_tcp_port 127.0.0.1 "$HTTP_BACKEND_PORT"  "$HTTP_BACKEND_PID"  "local HTTP backend"  20 || return 1
    wait_for_tcp_port 127.0.0.1 "$TCP_BACKEND_PORT"   "$TCP_BACKEND_PID"   "local TCP backend"   20 || return 1
    wait_for_tcp_port 127.0.0.1 "$UPDATE_SERVER_PORT" "$UPDATE_SERVER_PID" "local update server" 20 || return 1

    # Phase 4's update_url must NOT be a loopback literal: `geoip::init` (and
    # therefore `GeoIpUpdater::new` -> classify_literal_destination) runs
    # BEFORE `init_integration_testing_mode` in the gateway startup sequence
    # (edgion-gateway/src/cli/mod.rs: step 6b vs. step 10), so the
    # cfg!(test)-or-integration-testing-mode loopback escape hatch is NOT yet
    # active at that point and a `127.0.0.1` update_url is unconditionally
    # rejected ("update_url resolves to a blocked destination (loopback)"),
    # even with --integration-testing-mode on the command line. A private
    # (RFC1918) literal takes a different branch in classify_literal_destination
    # that is allowed unconditionally by default (EGRESS_BLOCK_PRIVATE
    # defaults to false), so binding the mirror on the host's real outbound
    # interface address sidesteps the ordering gap without touching product
    # code. This is a real gap worth a maintainer's attention, not a test
    # workaround for a test-only problem — see the Phase 4 comment and the
    # task report for the write-up.
    UPDATE_SERVER_HOST=$(ipconfig getifaddr en0 2>/dev/null || true)
    if [ -z "$UPDATE_SERVER_HOST" ]; then
        UPDATE_SERVER_HOST=$(ipconfig getifaddr en1 2>/dev/null || true)
    fi
    if [ -z "$UPDATE_SERVER_HOST" ]; then
        UPDATE_SERVER_HOST=$(hostname -I 2>/dev/null | awk '{print $1}')
    fi
    if [ -z "$UPDATE_SERVER_HOST" ]; then
        echo "FAIL: could not discover a private (non-loopback) IPv4 address for the update-mirror"
        return 1
    fi

    echo "PASS: local test backends up (http ${HTTP_BACKEND_PORT}, tcp ${TCP_BACKEND_PORT}, update-mirror ${UPDATE_SERVER_HOST}:${UPDATE_SERVER_PORT})"
}

# =============================================================================
# Minimal Controller (gRPC sync backend), seeded ONCE with every resource all
# four phases need. Only the Gateway's local `gateway.geoip` block differs
# between phases, so nothing here needs to change across restarts.
# =============================================================================
CTRL_TMPDIR=""
CTRL_PID=""

start_controller() {
    CTRL_TMPDIR=$(new_test_dir "controller")
    local CONFIG_DIR="${CTRL_TMPDIR}/config"
    local LOG_DIR="${CTRL_TMPDIR}/logs"
    local CONF_DIR="${CTRL_TMPDIR}/conf"
    mkdir -p "$CONFIG_DIR" "$LOG_DIR" "$CONF_DIR"

    if [ -d "${REPO_ROOT}/config/crd" ]; then
        cp -R "${REPO_ROOT}/config/crd/." "${CONFIG_DIR}/crd/"
    else
        echo "FAIL: CRD schema source not found: ${REPO_ROOT}/config/crd"
        return 1
    fi

    mkdir -p "${CONF_DIR}/cluster" "${CONF_DIR}/edgion-test" "${CONF_DIR}/edgion-default"
    local base="${REPO_ROOT}/edgion-tests/integration/conf/base"
    if [ ! -f "${base}/GatewayClass.yaml" ] || [ ! -f "${base}/EdgionGatewayConfig.yaml" ]; then
        echo "FAIL: base Gateway resources not found under ${base}"
        return 1
    fi
    cp "${base}/GatewayClass.yaml" "${CONF_DIR}/cluster/GatewayClass__public-gateway.yaml"
    cp "${base}/EdgionGatewayConfig.yaml" "${CONF_DIR}/cluster/EdgionGatewayConfig__example-gateway.yaml"
    # HTTPRoutes live in edgion-default, their Services in edgion-test: a
    # cross-namespace backendRef needs an explicit ReferenceGrant (Gateway API
    # semantics) or resolution fails closed with BackendServiceNotFound/RefDenied
    # -> 500 (edgion-gateway/src/backends/mod.rs::get_peer).
    if [ ! -f "${base}/ReferenceGrant_edgion-test_allow-cross-ns.yaml" ]; then
        echo "FAIL: base ReferenceGrant not found under ${base}"
        return 1
    fi
    cp "${base}/ReferenceGrant_edgion-test_allow-cross-ns.yaml" \
        "${CONF_DIR}/edgion-test/ReferenceGrant_edgion-test_allow-cross-ns.yaml"

    # --- Gateways -----------------------------------------------------------
    cat > "${CONF_DIR}/edgion-test/Gateway_edgion-test_geo-http-gw.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: geo-http-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: http
      protocol: HTTP
      port: ${GW_HTTP_PORT}
      allowedRoutes:
        namespaces:
          from: All
EOF

    cat > "${CONF_DIR}/edgion-test/Gateway_edgion-test_geo-tcp-deny-gw.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: geo-tcp-deny-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: tcp
      protocol: TCP
      port: ${GW_TCP_DENY_PORT}
EOF

    cat > "${CONF_DIR}/edgion-test/Gateway_edgion-test_geo-tcp-allow-gw.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: geo-tcp-allow-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: tcp
      protocol: TCP
      port: ${GW_TCP_ALLOW_PORT}
EOF

    # --- L4 backend (Service + EndpointSlice -> local tcp_backend.py) --------
    cat > "${CONF_DIR}/edgion-test/Service_edgion-test_geo-tcp-backend.yaml" <<EOF
apiVersion: v1
kind: Service
metadata:
  name: geo-tcp-backend
  namespace: edgion-test
spec:
  ports:
    - name: tcp
      port: ${TCP_BACKEND_PORT}
      protocol: TCP
      targetPort: ${TCP_BACKEND_PORT}
  type: ClusterIP
EOF

    cat > "${CONF_DIR}/edgion-test/EndpointSlice_edgion-test_geo-tcp-backend-slice.yaml" <<EOF
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: geo-tcp-backend-slice
  namespace: edgion-test
  labels:
    kubernetes.io/service-name: geo-tcp-backend
addressType: IPv4
ports:
  - name: tcp
    port: ${TCP_BACKEND_PORT}
    protocol: TCP
endpoints:
  - addresses:
      - "127.0.0.1"
    conditions:
      ready: true
EOF

    # --- L4 GeoIpLocation: one ESP per failOpen value, plus a CIDR-only ESP -
    # A bare `denyRuleGroups`/country rule (no CIDR) is enough to exercise the
    # Unavailable/db_unavailable branch: that branch is checked BEFORE any
    # rule needing resolved dimensions is consulted, so it fires purely from
    # "no database loaded" regardless of what the country list says.
    cat > "${CONF_DIR}/edgion-test/EdgionStreamPlugins_edgion-test_geo-tcp-esp-deny.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: geo-tcp-esp-deny
  namespace: edgion-test
spec:
  plugins:
    - type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: db-unavailable-deny
            rules:
              - source: DirectPeerIp
                key: country
                values: ["ZZ"]
EOF

    cat > "${CONF_DIR}/edgion-test/EdgionStreamPlugins_edgion-test_geo-tcp-esp-allow.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: geo-tcp-esp-allow
  namespace: edgion-test
spec:
  plugins:
    - type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: true
        denyRuleGroups:
          - name: db-unavailable-allow
            rules:
              - source: DirectPeerIp
                key: country
                values: ["ZZ"]
EOF

    # CIDR-only ESP: `key: cidr` never needs the GeoIP database, so this must
    # deny/allow purely by IP membership -- identically across every phase,
    # including Phase 1 (no database) with failOpen:false. `failOpen` is still
    # required by validation once a rule group is configured, but must never
    # actually be consulted for this config.
    cat > "${CONF_DIR}/edgion-test/EdgionStreamPlugins_edgion-test_geo-tcp-esp-cidr.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: geo-tcp-esp-cidr
  namespace: edgion-test
spec:
  plugins:
    - type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: cidr-only-deny
            rules:
              - source: DirectPeerIp
                key: cidr
                values: ["127.0.0.1/32"]
EOF

    cat > "${CONF_DIR}/edgion-test/Gateway_edgion-test_geo-tcp-cidr-gw.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: geo-tcp-cidr-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: tcp
      protocol: TCP
      port: ${GW_TCP_CIDR_PORT}
EOF

    cat > "${CONF_DIR}/edgion-test/TCPRoute_edgion-test_geo-tcp-deny-route.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: geo-tcp-deny-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "geo-tcp-esp-deny"
spec:
  parentRefs:
    - name: geo-tcp-deny-gw
      namespace: edgion-test
      sectionName: tcp
      port: ${GW_TCP_DENY_PORT}
  rules:
    - backendRefs:
        - name: geo-tcp-backend
          port: ${TCP_BACKEND_PORT}
          weight: 100
EOF

    cat > "${CONF_DIR}/edgion-test/TCPRoute_edgion-test_geo-tcp-allow-route.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: geo-tcp-allow-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "geo-tcp-esp-allow"
spec:
  parentRefs:
    - name: geo-tcp-allow-gw
      namespace: edgion-test
      sectionName: tcp
      port: ${GW_TCP_ALLOW_PORT}
  rules:
    - backendRefs:
        - name: geo-tcp-backend
          port: ${TCP_BACKEND_PORT}
          weight: 100
EOF

    cat > "${CONF_DIR}/edgion-test/TCPRoute_edgion-test_geo-tcp-cidr-route.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: geo-tcp-cidr-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "geo-tcp-esp-cidr"
spec:
  parentRefs:
    - name: geo-tcp-cidr-gw
      namespace: edgion-test
      sectionName: tcp
      port: ${GW_TCP_CIDR_PORT}
  rules:
    - backendRefs:
        - name: geo-tcp-backend
          port: ${TCP_BACKEND_PORT}
          weight: 100
EOF

    # --- HTTP backend (Service + EndpointSlice -> local http_backend.py) ----
    cat > "${CONF_DIR}/edgion-test/Service_edgion-test_geo-http-backend.yaml" <<EOF
apiVersion: v1
kind: Service
metadata:
  name: geo-http-backend
  namespace: edgion-test
spec:
  ports:
    - name: http
      port: ${HTTP_BACKEND_PORT}
      protocol: TCP
      targetPort: ${HTTP_BACKEND_PORT}
  type: ClusterIP
EOF

    cat > "${CONF_DIR}/edgion-test/EndpointSlice_edgion-test_geo-http-backend-slice.yaml" <<EOF
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: geo-http-backend-slice
  namespace: edgion-test
  labels:
    kubernetes.io/service-name: geo-http-backend
addressType: IPv4
ports:
  - name: http
    port: ${HTTP_BACKEND_PORT}
    protocol: TCP
endpoints:
  - addresses:
      - "127.0.0.1"
    conditions:
      ready: true
EOF

    # --- HTTP GeoIpLocation plugins ------------------------------------------
    # geo-bl403: DirectPeerIp/country==CN deny rule, failOpen false. Phase 1
    # (no database) must reject via the fail-closed db-unavailable branch,
    # checked BEFORE the country list is even consulted.
    cat > "${CONF_DIR}/edgion-default/EdgionPlugins_edgion-default_geo-bl403.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-bl403
  namespace: edgion-default
spec:
  requestPlugins:
    - enable: true
      type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: cn-deny
            rules:
              - source: DirectPeerIp
                key: country
                values: ["CN"]
EOF

    # geo-wl403: the whitelist-equivalent shape -- allowRuleGroups instead of
    # denyRuleGroups. A db-unavailable lookup is fail-closed exactly the same
    # way (checked before any rule group, deny or allow, is consulted), so
    # Phase 1 rejects through this route too, proving the fail-closed branch
    # is mode-independent.
    cat > "${CONF_DIR}/edgion-default/EdgionPlugins_edgion-default_geo-wl403.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-wl403
  namespace: edgion-default
spec:
  requestPlugins:
    - enable: true
      type: GeoIpLocation
      config:
        defaultAction: deny
        failOpen: false
        allowRuleGroups:
          - name: cn-allow
            rules:
              - source: DirectPeerIp
                key: country
                values: ["CN"]
EOF

    cat > "${CONF_DIR}/edgion-default/EdgionPlugins_edgion-default_geo-passopen.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-passopen
  namespace: edgion-default
spec:
  requestPlugins:
    - enable: true
      type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: true
        denyRuleGroups:
          - name: cn-deny-failopen
            rules:
              - source: DirectPeerIp
                key: country
                values: ["CN"]
EOF

    # geo-city: key=city, value=Beijing (127.0.0.1's city per
    # edgion-tests/geoip/README.md's address map). Inert (the dimension is
    # simply absent, ordinary missingness) against the Country fixture; live
    # (matches, denies) against the City fixture.
    cat > "${CONF_DIR}/edgion-default/EdgionPlugins_edgion-default_geo-city.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-city
  namespace: edgion-default
spec:
  requestPlugins:
    - enable: true
      type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: city-deny
            rules:
              - source: DirectPeerIp
                key: city
                values: ["Beijing"]
EOF

    # geo-cidr: `key: cidr` never needs the GeoIP database -- must deny/allow
    # purely by IP membership, identically across every phase, including
    # Phase 1 (no database) with failOpen:false. This is the CIDR-only-
    # immunity coverage (see this script's header comment).
    cat > "${CONF_DIR}/edgion-default/EdgionPlugins_edgion-default_geo-cidr.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-cidr
  namespace: edgion-default
spec:
  requestPlugins:
    - enable: true
      type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: cidr-deny
            rules:
              - source: DirectPeerIp
                key: cidr
                values: ["127.0.0.1/32"]
EOF

    for name in geo-bl403 geo-wl403 geo-passopen geo-city geo-cidr; do
        cat > "${CONF_DIR}/edgion-default/HTTPRoute_edgion-default_${name}-route.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: ${name}-route
  namespace: edgion-default
spec:
  parentRefs:
    - name: geo-http-gw
      namespace: edgion-test
  hostnames:
    - "${name}.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: ${name}
      backendRefs:
        - name: geo-http-backend
          namespace: edgion-test
          port: ${HTTP_BACKEND_PORT}
EOF
    done

    # conf_sync is mTLS by default: distribute the certs into the Controller's
    # work dir, then let the shared emitter write the transport block.
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

    if ! wait_for_url_200 \
            "http://127.0.0.1:${CTRL_PROBE_PORT}/ready" \
            "$CTRL_PID" \
            "controller /ready" 90; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    if ! wait_for_url_200 \
            "http://127.0.0.1:${CTRL_METRICS_PORT}/metrics" \
            "$CTRL_PID" \
            "controller metrics" 30; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    echo "PASS: backend Controller is ready (gRPC on 127.0.0.1:${CTRL_GRPC_PORT})"
}

# =============================================================================
# Gateway launch/teardown. `geoip_fn` is a shell function name; it is invoked
# AFTER GW_TMPDIR is set (so it may write files under it, e.g. copying a
# fixture into a writable location) and must print the `gateway: geoip: ...`
# YAML block to stdout.
# =============================================================================
GW_TMPDIR=""
GW_PID=""
GW_LOG=""
ACCESS_LOG=""

start_gateway() {
    local label="$1" geoip_fn="$2"
    GW_TMPDIR=$(new_test_dir "gateway-${label}")
    local LOG_DIR="${GW_TMPDIR}/logs"
    mkdir -p "$LOG_DIR"
    GW_LOG="$LOG_DIR/gateway.log"
    ACCESS_LOG="$LOG_DIR/edgion_access.log"

    conf_sync_copy_certs "$CTRL_TMPDIR" "$GW_TMPDIR"

    {
        cat <<EOF
work_dir: "."
logging:
  log_dir: "${LOG_DIR}"
metrics:
  listen: "127.0.0.1:${GW_METRICS_PORT}"
EOF
        "$geoip_fn"
    } > "${GW_TMPDIR}/gateway.yaml"
    conf_sync_security_block client >> "${GW_TMPDIR}/gateway.yaml"

    precheck_port "$GW_ADMIN_PORT"      "gateway admin"      || return 1
    precheck_port "$GW_PROBE_PORT"      "gateway probe"      || return 1
    precheck_port "$GW_METRICS_PORT"    "gateway metrics"    || return 1
    precheck_port "$GW_HTTP_PORT"       "gateway http"       || return 1
    precheck_port "$GW_TCP_DENY_PORT"   "gateway tcp deny"   || return 1
    precheck_port "$GW_TCP_ALLOW_PORT"  "gateway tcp allow"  || return 1
    precheck_port "$GW_TCP_CIDR_PORT"   "gateway tcp cidr"   || return 1

    echo "=== Starting gateway: ${label} ==="
    EDGION_GATEWAY_ADMIN_TOKEN="$TEST_TOKEN" \
    EDGION_ACCESS_LOG="$ACCESS_LOG" \
    EDGION_TEST_ACCESS_LOG_PATH="$ACCESS_LOG" \
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

    if ! wait_for_url_200 \
            "http://127.0.0.1:${GW_PROBE_PORT}/health" \
            "$GW_PID" \
            "gateway ${label} (/health)" 60; then
        echo "---- gateway log ----"
        cat "$GW_LOG"
        return 1
    fi
    if ! wait_for_tcp_port 127.0.0.1 "$GW_HTTP_PORT" "$GW_PID" "gateway ${label} HTTP listener" 60; then
        cat "$GW_LOG"; return 1
    fi
    if ! wait_for_tcp_port 127.0.0.1 "$GW_TCP_DENY_PORT" "$GW_PID" "gateway ${label} TCP-deny listener" 60; then
        cat "$GW_LOG"; return 1
    fi
    if ! wait_for_tcp_port 127.0.0.1 "$GW_TCP_ALLOW_PORT" "$GW_PID" "gateway ${label} TCP-allow listener" 60; then
        cat "$GW_LOG"; return 1
    fi
    if ! wait_for_tcp_port 127.0.0.1 "$GW_TCP_CIDR_PORT" "$GW_PID" "gateway ${label} TCP-cidr listener" 60; then
        cat "$GW_LOG"; return 1
    fi
    echo "PASS: gateway ${label} is up and serving"
}

stop_gateway() {
    [ -n "$GW_PID" ] && kill "$GW_PID" 2>/dev/null || true
    [ -n "$GW_PID" ] && wait "$GW_PID" 2>/dev/null || true
    [ -n "$GW_TMPDIR" ] && rm -rf "$GW_TMPDIR" || true
    GW_PID=""
    GW_TMPDIR=""
}

# =============================================================================
# Cleanup
# =============================================================================
cleanup() {
    stop_gateway
    [ -n "$CTRL_PID" ] && kill "$CTRL_PID" 2>/dev/null || true
    [ -n "$CTRL_PID" ] && wait "$CTRL_PID" 2>/dev/null || true
    [ -n "$CTRL_TMPDIR" ] && rm -rf "$CTRL_TMPDIR" || true
    [ -n "$HTTP_BACKEND_PID" ] && kill "$HTTP_BACKEND_PID" 2>/dev/null || true
    [ -n "$TCP_BACKEND_PID" ] && kill "$TCP_BACKEND_PID" 2>/dev/null || true
    [ -n "$UPDATE_SERVER_PID" ] && kill "$UPDATE_SERVER_PID" 2>/dev/null || true
    [ -n "$HTTP_BACKEND_PID" ] && wait "$HTTP_BACKEND_PID" 2>/dev/null || true
    [ -n "$TCP_BACKEND_PID" ] && wait "$TCP_BACKEND_PID" 2>/dev/null || true
    [ -n "$UPDATE_SERVER_PID" ] && wait "$UPDATE_SERVER_PID" 2>/dev/null || true
    [ -n "$SCRATCH_DIR" ] && rm -rf "$SCRATCH_DIR" || true
    [ -n "$TEST_WORK_ROOT" ] && rm -rf "$TEST_WORK_ROOT" || true
}
trap cleanup EXIT

# =============================================================================
# Phase 1: no database
# =============================================================================
DB_PATH_PHASE1=""
geoip_block_phase1() {
    DB_PATH_PHASE1="${GW_TMPDIR}/geoip/does-not-exist.mmdb"
    cat <<EOF
gateway:
  geoip:
    db_path: "${DB_PATH_PHASE1}"
EOF
}

run_phase1() {
    echo ""
    echo "=== Phase 1: no database ==="
    start_gateway "phase1-no-db" geoip_block_phase1 || return 1

    check "edgion_geoip_db_loaded == 0" "$(metric_value edgion_geoip_db_loaded)" "0"

    check "denyRuleGroups + failOpen:false rejects (db unavailable)" \
        "$(http_status -H 'Host: geo-bl403.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "403"
    check "allowRuleGroups + failOpen:false ALSO rejects (db unavailable, mode-independent)" \
        "$(http_status -H 'Host: geo-wl403.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "403"
    check "failOpen:true drops the rule group and passes (db unavailable)" \
        "$(http_status -H 'Host: geo-passopen.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "200"

    # CIDR-only-immunity (HTTP): key: cidr never touches the GeoIP database,
    # so it must behave identically here (no database at all) as it would
    # with one loaded -- denying the matching source and admitting everyone
    # else, never falling into the fail-open/fail-closed branch above.
    check "key: cidr denies its matching source even with no database loaded" \
        "$(http_status -H 'Host: geo-cidr.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "403"
    check "key: cidr admits a non-matching source even with no database loaded" \
        "$(http_status --interface 127.0.0.5 -H 'Host: geo-cidr.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "200"

    local deny_before
    deny_before=$(metric_value edgion_geoip_denied_total 'layer="tcp"' 'reason="db_unavailable"')

    check "L4 failOpen:false denies (db unavailable)" "$(tcp_probe "$GW_TCP_DENY_PORT")" "CLOSED"
    local deny_after
    deny_after=$(metric_value edgion_geoip_denied_total 'layer="tcp"' 'reason="db_unavailable"')
    check_true "edgion_geoip_denied_total{layer=tcp,reason=db_unavailable} incremented" \
        "$(( $(printf '%.0f' "$deny_after") > $(printf '%.0f' "$deny_before") ? 1 : 0 ))"

    check "L4 failOpen:true admits (db unavailable)" "$(tcp_probe "$GW_TCP_ALLOW_PORT")" "ECHO"

    # CIDR-only-immunity (L4): same proof as the HTTP checks above, at the
    # stream-plugin layer.
    check "L4 key: cidr denies its matching source even with no database loaded" \
        "$(tcp_probe "$GW_TCP_CIDR_PORT" "127.0.0.1")" "CLOSED"
    check "L4 key: cidr admits a non-matching source even with no database loaded" \
        "$(tcp_probe "$GW_TCP_CIDR_PORT" "127.0.0.5")" "ECHO"

    stop_gateway
}

# =============================================================================
# Phase 2: Country-tier
# =============================================================================
geoip_block_phase2() {
    cat <<EOF
gateway:
  geoip:
    db_path: "${COUNTRY_FIXTURE}"
EOF
}

run_phase2() {
    echo ""
    echo "=== Phase 2: Country-tier database ==="
    start_gateway "phase2-country" geoip_block_phase2 || return 1

    check "edgion_geoip_db_loaded == 1" "$(metric_value edgion_geoip_db_loaded)" "1"

    # 127.0.0.1 -> CN (matches the blacklist rule) -> denied.
    check "country rule denies a matching source (127.0.0.1=CN)" \
        "$(http_status -H 'Host: geo-bl403.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "403"
    # 127.0.0.5 -> RU (does not match) -> passes through to the backend.
    check "country rule passes a non-matching source (127.0.0.5=RU)" \
        "$(http_status --interface 127.0.0.5 -H 'Host: geo-bl403.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "200"

    # City-dimension rule against a Country-tier database. There is no
    # "database tier" concept in the runtime (2026-08-15 review, final fix
    # wave): a `city` rule against a database that never carries city data
    # simply has no value to compare -- ordinary "missing value" -- and
    # `GeoRule::matches`'s fixed "absent dimension => no match" (`geo_rule.rs`
    # has no `onMissing` knob) means the rule never matches (200). There is no
    # separate warning status; prove the real contract directly from the
    # access log's geo block instead: the lookup succeeded (status=found,
    # country resolved) but no `city` value was ever recorded, for this
    # request or any other in this phase, because the Country-tier fixture
    # never carries one. This 200 (inert) result also doubles as the positive
    # proof that this phase actually loaded the Country-tier fixture and not
    # the City-tier one -- there is no admin-endpoint snapshot to read
    # instead; Phase 3/4 use the same rule's opposite outcome (403, live) as
    # their proof of loading the City-tier fixture.
    check "city rule against Country db is inert (not denied)" \
        "$(http_status -H 'Host: geo-city.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "200"
    if wait_for_access_log_contains '"status":"found"'; then
        echo "PASS: access log geo block reports a found lookup (country resolved)"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        echo "FAIL: access log never recorded a found geo lookup"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
    if grep -q '"city"' "$ACCESS_LOG"; then
        echo "FAIL: access log recorded a city value against the Country-tier fixture (it should never carry one)"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    else
        echo "PASS: access log never recorded a city value -- the predicate is correctly absent, no onMissing knob needed"
        PASS_COUNT=$((PASS_COUNT + 1))
    fi

    stop_gateway
}

# =============================================================================
# Phase 3: City-tier — coordinates are emitted whenever the record carries them
# =============================================================================
geoip_block_phase3() {
    cat <<EOF
gateway:
  geoip:
    db_path: "${CITY_FIXTURE}"
EOF
}

run_phase3() {
    echo ""
    echo "=== Phase 3: City-tier database (coordinates emitted from the record) ==="
    start_gateway "phase3-city-coords" geoip_block_phase3 || return 1

    # Positive proof this phase actually loaded the City-tier fixture (there is
    # no admin-endpoint snapshot to read instead): geo-city, inert (200)
    # against the Country-tier fixture in Phase 2, becomes live (403) here.
    check "city rule becomes live against City-tier db (proves City-tier is loaded)" \
        "$(http_status -H 'Host: geo-city.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "403"

    # geo-wl403 is the whitelist-shaped allowRuleGroups/country==CN config:
    # with a real database loaded, CN (127.0.0.1) matches the allow group and
    # passes (200). What we actually care about here is only the access-log
    # geo block: this route's plugin queries `DirectPeerIp`, so the
    # `direct_peer` slot is populated regardless of the decision outcome (entry.rs::
    # from_context reads whatever the plugin already resolved — no global
    # switch, no forced lookup) — this route is just a convenient one
    # guaranteed to reach the backend with a loaded database.
    check "request from a coord-bearing source (127.0.0.1=CN/Beijing) passes" \
        "$(http_status -H 'Host: geo-wl403.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "200"

    if wait_for_access_log_contains '"coord":"\*\*\*"'; then
    echo "PASS: access log has geo.direct_peer.coord, masked (\"***\")"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
    echo "FAIL: access log missing a masked geo.direct_peer.coord field"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi

    stop_gateway
}

# =============================================================================
# Phase 4: updater hot-swap (Country -> City)
# =============================================================================
DB_PATH_PHASE4=""
geoip_block_phase4() {
    mkdir -p "${GW_TMPDIR}/geoip"
    DB_PATH_PHASE4="${GW_TMPDIR}/geoip/db.mmdb"
    cp "$COUNTRY_FIXTURE" "$DB_PATH_PHASE4"
    cat <<EOF
gateway:
  geoip:
    db_path: "${DB_PATH_PHASE4}"
    update_url: "http://${UPDATE_SERVER_HOST}:${UPDATE_SERVER_PORT}/edgion-test-city.mmdb"
EOF
}

run_phase4() {
    echo ""
    echo "=== Phase 4: updater hot-swap (Country -> City) ==="
    start_gateway "phase4-hot-swap" geoip_block_phase4 || return 1

    # Read immediately after start_gateway returns: the local update server
    # sleeps 3s before its first response byte, and the updater's own initial
    # fetch is additionally jittered 0-5s (updater.rs sleep_with_jitter), so
    # this read is deterministically pre-swap rather than racing it. There is
    # no admin-endpoint snapshot to read instead (removed 2026-08-15 review):
    # the city rule's own inert/live flip is both the setup check and the
    # positive proof that the hot-swap actually replaced the database, exactly
    # like Phase 2/3 use it to prove which fixture tier is loaded.
    local initial_epoch
    initial_epoch=$(metric_value edgion_geoip_db_build_epoch_seconds)

    check "city rule is initially inert (Country db, pre-swap)" \
        "$(http_status -H 'Host: geo-city.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "200"

    wait_for_http_status "http://127.0.0.1:${GW_HTTP_PORT}/" "geo-city.example.com" "403" 30
    local post_epoch
    post_epoch=$(metric_value edgion_geoip_db_build_epoch_seconds)

    check "city rule becomes live (City db, post-swap — proves the hot-swap actually replaced the database)" \
        "$(http_status -H 'Host: geo-city.example.com' "http://127.0.0.1:${GW_HTTP_PORT}/")" "403"

    echo "INFO: build_epoch before=${initial_epoch} after=${post_epoch} — both fixtures share one" \
         "pinned constant (edgion-tests/geoip/generate.py monkeypatches time.time once, before" \
         "writing either file), so equality here is expected fixture behavior, not a flip. Not" \
         "asserted either way — see this script's header comment."

    if [ -f "${DB_PATH_PHASE4}.meta" ]; then
        echo "PASS: ${DB_PATH_PHASE4}.meta was written"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        echo "FAIL: ${DB_PATH_PHASE4}.meta is missing"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
    if [ ! -f "${DB_PATH_PHASE4}.tmp" ]; then
        echo "PASS: no ${DB_PATH_PHASE4}.tmp left behind"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        echo "FAIL: ${DB_PATH_PHASE4}.tmp still present"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi

    stop_gateway
}

# =============================================================================
# Entry point
# =============================================================================
main() {
    init_test_work_root
    precheck_disk_space "$TEST_WORK_ROOT" || exit 2
    start_backends
    start_controller
    run_phase1
    run_phase2
    run_phase3
    run_phase4

    echo ""
    echo "=== GeoIP database-state preflight: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
    if [ "$FAIL_COUNT" -gt 0 ]; then
        exit 1
    fi
}

main
