#!/usr/bin/env bash
# =============================================================================
# Controller Plugin Policy (allowlist / denylist) — end-to-end integration test.
#
# Design: docs/superpowers/specs/2026-07-17-controller-plugin-policy-design.md
#
# WHY STANDALONE (cannot reuse the shared run_integration.sh stack):
#   The policy lives in the CONTROLLER config file (validation.plugin_policy) and
#   is compiled once at startup — a policy change REQUIRES a controller restart
#   (spec §3). The shared harness boots a single controller from the checked-in
#   config with no policy and never restarts it, so it can neither inject a
#   policy nor exercise the relaxation (restart-without-policy) path. This script
#   therefore boots its own minimal stack (test_server + file_system controller +
#   gateway) on dedicated 583xx ports and restarts ONLY the controller between
#   phases. It mirrors test_fips_policy.sh (process-global config that must not
#   leak into the shared gateway) and test_resource_abnormal_metrics.sh (custom
#   controller config). The gateway stays up across restarts and re-syncs a fresh
#   full snapshot on reconnect — exactly the propagation the feature relies on
#   (spec §7).
#
# ONE EdgionPlugins(Mock) resource drives three contrasting outcomes:
#   Phase 1  denylist http/Mock -> block(451)   : request rejected at ingress
#            + stream/IpRestriction -> block     : TCP connection closed
#   Phase 2  denylist http/Mock -> bypass        : Mock skipped, upstream reached
#   Phase 3  policy absent (relaxed)             : Mock runs again, condition gone
#
# Assertions map to spec §8 (integration bullets):
#   - block: configured status (451) + fixed body, PluginPolicyBlock/http/Mock in
#     the access log, upstream not reached (451, never the mock/upstream body).
#   - bypass: 200 to the real upstream (mock effect absent) + PluginPolicyBypass.
#   - stream block: connection closed.
#   - status: PluginPolicyRestricted=True present AND Accepted=True (file-mode
#     .status sidecar; the controller abnormal-resource metric is NOT asserted —
#     file mode cannot surface it, spec §6/§8; that is unit-covered in Task 5).
#   - relaxation: after a restart without the policy, PluginPolicyRestricted is
#     actively removed from the status (regression for spec §4).
#   - stream-block positive control: same phase-3 (no-policy) stack, TCP probe
#     must ECHO (the IpRestriction plugin itself admits 127.0.0.1), proving
#     Phase 1's CLOSED came from the policy gate and not a broken listener.
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
FIXTURE_DIR="${REPO_ROOT}/edgion-tests/integration/conf/PluginPolicy"

# Dedicated port block (avoids the 12xxx shared stack and 58310-58317 used by
# the FIPS preflight).
CTRL_ADMIN_PORT=58320
CTRL_GRPC_PORT=58321
CTRL_PROBE_PORT=58322
GW_ADMIN_PORT=58323
GW_PROBE_PORT=58324
GW_METRICS_PORT=58325
CTRL_METRICS_PORT=58326
GW_HTTP_PORT=58330
GW_TCP_PORT=58331
# test_server backends (same fixed ports the fixtures reference).
TS_HTTP_PORT=30001
TS_TCP_PORT=30010

HTTP_HOST="pp-mock.example.com"
MOCK_MARKER="PP-MOCK-TERMINATED"
BLOCK_BODY_FRAGMENT="plugin disabled by policy"

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
if ! command -v python3 >/dev/null 2>&1; then
    log_err "python3 not found (required for the TCP stream probe)"
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

# The gateway's LocalFile access-log sink sheds to unhealthy — and then silently
# drops every access-log entry — once the log volume reaches
# `disk_usage_threshold_percent` (product default 90, see
# `default_disk_usage_threshold_percent` in edgion-resources/src/output.rs). This
# scenario asserts on access-log content (PluginPolicyBlock / PluginPolicyBypass),
# so on a >=90%-full host those assertions would fail for a reason unrelated to the
# feature under test. Detect the unfit environment up front and abort loudly with a
# clear diagnostic, rather than working around the product's disk check or letting it
# surface later as a confusing content-assertion failure. We do NOT override the
# product threshold — the disk check is correct and must keep its production default.
readonly ACCESS_LOG_DISK_THRESHOLD_PERCENT=90

precheck_disk_space() {
    local dir="$1"
    # Replicate the gateway's OWN disk_usage_percent() exactly, rather than trust
    # df's Capacity column: the gateway computes usage = (total - avail) / total,
    # integer-truncated (edgion-gateway .../local_file/data_sender_impl.rs), whereas
    # macOS df's Capacity uses used/(used+avail) rounded up — a different denominator
    # that can disagree by ~1% near the boundary. `df -P` reports 512-byte blocks:
    # $2 = total (statvfs f_blocks), $4 = available to unprivileged (f_bavail).
    local pct
    pct=$(df -P "$dir" 2>/dev/null | awk 'NR==2 && $2+0>0 { printf "%d", (($2-$4)*100)/$2 }')
    if [ -z "$pct" ]; then
        log_warn "could not determine disk usage for $dir; skipping disk precheck"
        return 0
    fi
    # The sink is healthy iff usage < threshold, so it sheds at usage >= threshold.
    if [ "$pct" -ge "$ACCESS_LOG_DISK_THRESHOLD_PERCENT" ]; then
        log_err "disk for $dir computes to ${pct}% full by the gateway's own formula (sheds at >= ${ACCESS_LOG_DISK_THRESHOLD_PERCENT}%)"
        log_err "the gateway would silently drop access-log entries, so this scenario's log assertions cannot pass"
        log_err "free enough space to drop BELOW ${ACCESS_LOG_DISK_THRESHOLD_PERCENT}% and re-run — this is an environment issue, not a plugin-policy failure"
        return 1
    fi
    return 0
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
# Working directory / process handles
# =============================================================================
WORK_DIR=""
CTRL_PID=""
GW_PID=""
TS_PID=""
GW_LOG=""
CTRL_LOG=""
ACCESS_LOG=""
CONF_DIR=""
PP_STATUS_FILE=""
TEST_PASSED=0

cleanup() {
    log_section "Cleanup"
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
# test_server (HTTP echo on 30001, TCP echo on 30010)
# =============================================================================
start_test_server() {
    log_section "Start test_server"
    precheck_port "$TS_HTTP_PORT" "test_server http" || return 1
    precheck_port "$TS_TCP_PORT"  "test_server tcp"  || return 1
    "$TEST_SERVER_BIN" \
        --http-ports "$TS_HTTP_PORT" \
        --tcp-port "$TS_TCP_PORT" \
        --log-level info \
        > "${WORK_DIR}/logs/test_server.log" 2>&1 &
    TS_PID=$!
    wait_for_tcp_port 127.0.0.1 "$TS_HTTP_PORT" "$TS_PID" "test_server http" 30 || return 1
    wait_for_url_200 "http://127.0.0.1:${TS_HTTP_PORT}/health" "$TS_PID" "test_server /health" 15 || return 1
    log_ok "test_server up (http ${TS_HTTP_PORT}, tcp ${TS_TCP_PORT})"
}

# =============================================================================
# Controller config generation: one file per policy phase, same work/conf dir.
# =============================================================================
write_controller_config() {
    local out="$1" policy_block="$2"   # policy_block = yaml snippet under validation, or empty
    cat > "$out" <<EOF
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
  conf_dir: "${CONF_DIR}"

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200

${policy_block}
EOF
    conf_sync_security_block server >> "$out"
}

# Kill any running controller and (re)start it with the given config file.
# The gateway (if already up) reconnects to the same gRPC port and re-syncs.
restart_controller() {
    local config_file="$1" label="$2"
    if [ -n "$CTRL_PID" ] && kill -0 "$CTRL_PID" 2>/dev/null; then
        kill "$CTRL_PID" 2>/dev/null || true
        wait "$CTRL_PID" 2>/dev/null || true
    fi
    # Give the OS a moment to release all four Controller ports. A stale bind
    # on any one of them must fail this phase before a new process starts.
    CTRL_PID=""
    for _ in $(seq 1 20); do
        if ! nc -z 127.0.0.1 "$CTRL_GRPC_PORT" 2>/dev/null \
           && ! nc -z 127.0.0.1 "$CTRL_ADMIN_PORT" 2>/dev/null \
           && ! nc -z 127.0.0.1 "$CTRL_PROBE_PORT" 2>/dev/null \
           && ! nc -z 127.0.0.1 "$CTRL_METRICS_PORT" 2>/dev/null; then
            break
        fi
        sleep 1
    done
    precheck_port "$CTRL_GRPC_PORT" "controller gRPC" || return 1
    precheck_port "$CTRL_ADMIN_PORT" "controller admin" || return 1
    precheck_port "$CTRL_PROBE_PORT" "controller probe" || return 1
    precheck_port "$CTRL_METRICS_PORT" "controller metrics" || return 1
    log_info "starting controller (${label})"
    "$CONTROLLER_BIN" \
        -c "$config_file" \
        --work-dir "$WORK_DIR" \
        > "${WORK_DIR}/logs/controller.${label}.log" 2>&1 &
    CTRL_PID=$!
    CTRL_LOG="${WORK_DIR}/logs/controller.${label}.log"
    if ! wait_for_url_200 "http://127.0.0.1:${CTRL_PROBE_PORT}/ready" "$CTRL_PID" "controller /ready (${label})" 60; then
        tail -40 "$CTRL_LOG" 2>/dev/null || true
        return 1
    fi
    wait_for_url_200 "http://127.0.0.1:${CTRL_METRICS_PORT}/metrics" \
        "$CTRL_PID" "controller metrics (${label})" 30 || return 1
    log_ok "controller ready (${label})"
}

# =============================================================================
# Gateway
# =============================================================================
start_gateway() {
    log_section "Start gateway"
    # Controller and gateway share this work dir, so conf_sync_distribute_certs
    # (run in setup) already placed the primary/secondary sets under
    # WORK_DIR/certs/grpc — both ends read the same CA, nothing to copy.
    ACCESS_LOG="${WORK_DIR}/logs/edgion_access.log"
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
    precheck_port "$GW_HTTP_PORT"    "gateway http"    || return 1
    precheck_port "$GW_TCP_PORT"     "gateway tcp"     || return 1

    EDGION_ACCESS_LOG="$ACCESS_LOG" \
    EDGION_TEST_ACCESS_LOG_PATH="$ACCESS_LOG" \
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
    wait_for_url_200 "http://127.0.0.1:${GW_PROBE_PORT}/health" "$GW_PID" "gateway /health" 60 || { tail -40 "$GW_LOG"; return 1; }
    wait_for_tcp_port 127.0.0.1 "$GW_HTTP_PORT" "$GW_PID" "gateway HTTP listener" 60 || { tail -40 "$GW_LOG"; return 1; }
    wait_for_tcp_port 127.0.0.1 "$GW_TCP_PORT" "$GW_PID" "gateway TCP listener" 60 || { tail -40 "$GW_LOG"; return 1; }
    log_ok "gateway up (http ${GW_HTTP_PORT}, tcp ${GW_TCP_PORT})"
}

# =============================================================================
# Probes
# =============================================================================
# HTTP probe: prints "<code>\t<body>" for a unique path (so the access-log line
# is greppable by path).
http_probe() {
    local path="$1"
    curl -s -m 5 -o /tmp/pp_body.$$ -w "%{http_code}" \
        -H "Host: ${HTTP_HOST}" "http://127.0.0.1:${GW_HTTP_PORT}${path}" 2>/dev/null || echo "000"
}
http_body() { cat /tmp/pp_body.$$ 2>/dev/null || true; }

# TCP probe: connect, send bytes, try to read an echo. Prints "ECHO" if bytes
# came back (connection served), "CLOSED" if the peer closed with no data.
tcp_probe() {
    python3 - "$GW_TCP_PORT" <<'PY'
import socket, sys
port = int(sys.argv[1])
try:
    s = socket.create_connection(("127.0.0.1", port), timeout=4)
    s.settimeout(4)
    s.sendall(b"pp-ping")
    data = s.recv(64)
    s.close()
    print("ECHO" if data else "CLOSED")
except (ConnectionResetError, BrokenPipeError):
    print("CLOSED")
except Exception as e:
    print("ERR:%s" % e)
PY
}

# Find the LAST access-log line that contains ALL the given needles, then
# succeed if such a line exists. NOTE: in integration-testing mode the access
# log redacts the request path (`"path":"***"`) and client addresses, so we
# CANNOT anchor on a per-probe path. We anchor on the policy marker itself
# (`PluginPolicyBlock` / `PluginPolicyBypass`), which is unique to the phase.
access_log_line_has() {
    local label="$1"; shift
    local matches
    matches=$(cat "$ACCESS_LOG" 2>/dev/null || true)
    local needle
    for needle in "$@"; do
        matches=$(printf '%s\n' "$matches" | grep -F -- "$needle" || true)
    done
    local line
    line=$(printf '%s\n' "$matches" | tail -1)
    if [ -z "$line" ]; then
        log_err "no access-log line contains all of: $* (${label})"
        return 1
    fi
    return 0
}

# Print the status of a named condition ("True"/"False"), or "ABSENT" if the
# condition is not present, or "MISSING" if the file does not exist. Parses the
# file-mode `.status` sidecar without PyYAML (serde emits fields in struct order,
# `status:` immediately before `type:` within each condition list item).
condition_status() {
    local file="$1" want="$2"
    [ -f "$file" ] || { echo "MISSING"; return 0; }
    awk -v want="$want" '
        /^- / { st="" }
        /^[[:space:]]*status:/ { s=$2; gsub(/['\''"]/,"",s); st=s }
        /^[[:space:]]*type:/   { t=$2; if (t==want) { print st; found=1; exit } }
        END { if (!found) print "ABSENT" }
    ' "$file"
}

# Poll until the EdgionPlugins(pp-mock) status has (present=1) / lacks
# (present=0) a PluginPolicyRestricted=True condition.
wait_status_restricted() {
    local want_present="$1" tries="${2:-40}"
    local i st
    for i in $(seq 1 "$tries"); do
        st=$(condition_status "$PP_STATUS_FILE" PluginPolicyRestricted)
        if [ "$want_present" = "1" ] && [ "$st" = "True" ]; then return 0; fi
        if [ "$want_present" = "0" ] && { [ "$st" = "ABSENT" ] || [ "$st" = "MISSING" ]; }; then return 0; fi
        sleep 1
    done
    return 1
}

# =============================================================================
# Setup
# =============================================================================
setup() {
    log_section "Setup"
    # Place the work dir (and thus the gateway's access-log file) under the
    # repo-local tmp tree, on the same volume the shared harness uses
    # (run_integration.sh → ${PROJECT_ROOT}/tmp/integration_testing) — NOT the
    # system TMPDIR, which can be a small/near-full internal volume even when the
    # repo lives on a roomy external disk. tmp/ is gitignored.
    local work_base="${REPO_ROOT}/tmp/integration_testing"
    mkdir -p "$work_base"
    WORK_DIR=$(mktemp -d "${work_base}/pp_integration.XXXXXX")
    CONF_DIR="${WORK_DIR}/conf"
    # File-mode status sidecar for the EdgionPlugins(pp-mock) resource.
    PP_STATUS_FILE="${CONF_DIR}/EdgionPlugins_edgion-test_pp-mock.yaml.status"
    mkdir -p "${WORK_DIR}/logs" "${WORK_DIR}/config" "$CONF_DIR"
    log_info "WORK_DIR=$WORK_DIR"

    # CRD schemas reachable (non-K8s mode loads them from <work_dir>/config/crd).
    ln -s "${REPO_ROOT}/config/crd" "${WORK_DIR}/config/crd"

    # conf_sync mTLS certs under WORK_DIR (server + client share one CA).
    conf_sync_distribute_certs "$WORK_DIR"

    # Seed the watched conf dir with the PluginPolicy fixtures. Every fixture is
    # named <Kind>_<namespace>_<name>.yaml (cluster-scoped uses a double
    # underscore for the empty namespace, e.g. GatewayClass__public-gateway.yaml)
    # and holds exactly ONE resource, as required by the file_system conf_center.
    # The GatewayClass ships as a local fixture (no parametersRef), so no
    # EdgionGatewayConfig is needed.
    cp "${FIXTURE_DIR}"/*.yaml "${CONF_DIR}/"
    log_ok "seeded $(ls "${CONF_DIR}" | wc -l | tr -d ' ') fixture files"

    # Three controller configs sharing one conf/work dir.
    write_controller_config "${WORK_DIR}/config/controller.block.yaml" \
"validation:
  plugin_policy:
    mode: denylist
    default_action: block
    default_block_status: 503
    plugins:
      - name: http/Mock
        action: block
        block_status: 451
      - name: stream/IpRestriction
        action: block"
    write_controller_config "${WORK_DIR}/config/controller.bypass.yaml" \
"validation:
  plugin_policy:
    mode: denylist
    plugins:
      - name: http/Mock
        action: bypass"
    write_controller_config "${WORK_DIR}/config/controller.none.yaml" ""
    log_ok "wrote 3 controller configs (block / bypass / none)"
}

# =============================================================================
# Phases
# =============================================================================
FAILED=0
fail() { log_err "$1"; FAILED=1; }

phase1_block() {
    log_section "Phase 1: denylist block (http/Mock -> 451, stream/IpRestriction -> close)"
    # The block controller + gateway are already running (started in main), so
    # this phase asserts against the live stack directly.

    # Poll the data plane until the block takes effect (fresh sync).
    local code i
    for i in $(seq 1 60); do
        code=$(http_probe "/pp-block-probe")
        [ "$code" = "451" ] && break
        sleep 1
    done
    code=$(http_probe "/pp-block-probe")
    local body; body=$(http_body)
    if [ "$code" = "451" ]; then log_ok "HTTP block returned 451"; else fail "expected 451, got $code (body: $body)"; fi
    if echo "$body" | grep -qF "$BLOCK_BODY_FRAGMENT"; then log_ok "block body carries fixed message"; else fail "block body missing '$BLOCK_BODY_FRAGMENT' (got: $body)"; fi
    if echo "$body" | grep -qF "$MOCK_MARKER"; then fail "mock/upstream body leaked on block (upstream reached?)"; else log_ok "mock/upstream body absent (upstream not reached)"; fi

    # Access log: PluginPolicyBlock + http/Mock on the blocked request line.
    if access_log_line_has "block" "PluginPolicyBlock" "http/Mock"; then
        log_ok "access log has PluginPolicyBlock + http/Mock"
    else
        fail "access log missing PluginPolicyBlock/http/Mock"
    fi

    # Stream block: TCP connection closed.
    local tcp; tcp=$(tcp_probe)
    if [ "$tcp" = "CLOSED" ]; then log_ok "stream connection closed (policy block)"; else fail "expected CLOSED TCP, got '$tcp'"; fi

    # Status: PluginPolicyRestricted=True present AND Accepted untouched (=True).
    if wait_status_restricted 1 40; then
        log_ok "status has PluginPolicyRestricted=True (file: ${PP_STATUS_FILE})"
        local acc; acc=$(condition_status "$PP_STATUS_FILE" Accepted)
        if [ "$acc" = "True" ]; then
            log_ok "Accepted=True (unaffected by policy)"
        else
            fail "Accepted=${acc} in ${PP_STATUS_FILE} (spec §8 requires Accepted unaffected by the policy)"
        fi
    else
        fail "PluginPolicyRestricted=True never appeared in status"
    fi
}

phase2_bypass() {
    log_section "Phase 2: denylist bypass (http/Mock skipped -> upstream reached)"
    restart_controller "${WORK_DIR}/config/controller.bypass.yaml" "bypass" || { fail "controller(bypass) start"; return; }

    local code i body
    for i in $(seq 1 60); do
        code=$(http_probe "/pp-bypass-probe")
        body=$(http_body)
        # bypass = 200 AND the mock marker is gone (real upstream answered).
        if [ "$code" = "200" ] && ! echo "$body" | grep -qF "$MOCK_MARKER"; then break; fi
        sleep 1
    done
    code=$(http_probe "/pp-bypass-probe"); body=$(http_body)
    if [ "$code" = "200" ]; then log_ok "HTTP bypass returned 200"; else fail "expected 200, got $code (body: $body)"; fi
    if echo "$body" | grep -qF "$MOCK_MARKER"; then fail "mock marker present -> Mock still executed (not bypassed)"; else log_ok "mock effect absent -> request reached real upstream"; fi
    if access_log_line_has "bypass" "PluginPolicyBypass" "http/Mock"; then
        log_ok "access log has PluginPolicyBypass + http/Mock"
    else
        fail "access log missing PluginPolicyBypass/http/Mock"
    fi
}

phase3_relaxation() {
    log_section "Phase 3: policy removed (relaxation -> condition removed, Mock runs)"
    restart_controller "${WORK_DIR}/config/controller.none.yaml" "none" || { fail "controller(none) start"; return; }

    # Status: PluginPolicyRestricted actively removed.
    if wait_status_restricted 0 40; then
        log_ok "PluginPolicyRestricted condition removed after relaxation"
    else
        fail "PluginPolicyRestricted condition still present after policy removal"
    fi

    # Data plane: Mock runs again (marker body returns, upstream not reached).
    local code i body
    for i in $(seq 1 60); do
        code=$(http_probe "/pp-relaxed-probe"); body=$(http_body)
        if [ "$code" = "200" ] && echo "$body" | grep -qF "$MOCK_MARKER"; then break; fi
        sleep 1
    done
    code=$(http_probe "/pp-relaxed-probe"); body=$(http_body)
    if [ "$code" = "200" ] && echo "$body" | grep -qF "$MOCK_MARKER"; then
        log_ok "Mock executes again after relaxation (marker body returned)"
    else
        fail "expected 200 + mock marker after relaxation, got $code (body: $body)"
    fi

    # Positive control for Phase 1's stream-block assertion: with no policy in
    # effect, the connection-stage IpRestriction plugin (pp-ipr) itself admits
    # 127.0.0.1 (allow: 127.0.0.0/8 + defaultAction: allow), so the SAME TCP
    # listener/route must now serve the probe with an echo round-trip. This
    # proves Phase 1's CLOSED came from the policy gate, not from a broken
    # listener/route/plugin — without this control, any wrong-reason close in
    # Phase 1 (e.g. a dead listener) would have passed silently.
    #
    # Poll instead of single-shot: the gateway relists each resource kind on an
    # INDEPENDENT exponential backoff (2s..32s max + jitter, see
    # edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs). Across this
    # test's repeated controller restarts, EdgionStreamPlugins can sit in a 32s
    # backoff while EdgionPlugins (HTTP) has already relisted — so the Mock
    # recovery above does NOT imply the stream-plugin store has shed the
    # phase-1 Block stamp yet. 60x1s comfortably covers the 32s+jitter worst
    # case, matching the Mock poll loop above.
    local tcp i
    for i in $(seq 1 60); do
        tcp=$(tcp_probe)
        if [ "$tcp" = "ECHO" ]; then break; fi
        sleep 1
    done
    if [ "$tcp" = "ECHO" ]; then
        log_ok "stream connection echoes with no policy (positive control for Phase 1's CLOSED)"
    else
        fail "expected ECHO with no policy (positive control), got '$tcp'"
    fi
}

# =============================================================================
# Entry point
# =============================================================================
main() {
    log_section "Controller Plugin Policy integration test"
    # The work dir (and thus the gateway's access-log file) lands under
    # ${REPO_ROOT}/tmp (see setup). Abort with a distinct code (2 = environment
    # unfit, vs 1 = test failure) if THAT volume is already over the access-log
    # shedding threshold, so a full disk reads as an env problem, not a
    # plugin-policy regression.
    precheck_disk_space "$REPO_ROOT" || exit 2
    setup
    start_test_server
    # First controller boot (block policy) BEFORE the gateway, so the gateway
    # syncs stamped resources on its initial connect.
    restart_controller "${WORK_DIR}/config/controller.block.yaml" "block" || { log_err "initial controller start failed"; exit 1; }
    start_gateway

    # Phase 1 re-runs the block assertions against the already-running stack.
    phase1_block
    phase2_bypass
    phase3_relaxation

    rm -f /tmp/pp_body.$$ 2>/dev/null || true

    log_section "Result"
    if [ "$FAILED" -eq 0 ]; then
        TEST_PASSED=1
        log_ok "ALL PLUGIN-POLICY SCENARIOS PASSED"
        exit 0
    else
        log_err "PLUGIN-POLICY TEST FAILED (see logs under $WORK_DIR)"
        exit 1
    fi
}

main "$@"
