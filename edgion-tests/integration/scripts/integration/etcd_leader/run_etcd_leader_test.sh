#!/usr/bin/env bash
# =============================================================================
# run_etcd_leader_test.sh — Controller-side etcd leader-election failover test
# (Plan A Task 14; spec §10 "Integration" items 1 and 3 without a gateway).
#
# Topology: 1 etcd + 2 edgion-controller processes in etcd mode sharing one
# key_prefix. Asserts:
#   (a) exactly one controller becomes leader (/ready 200; the other 503)
#   (b) the follower's Admin API business routes return 409 naming the
#       leader's advertised admin_addr (spec §6.2)
#   (c) exactly one leader key exists under the election prefix, lease-attached
#   (d) kill -9 the leader -> the standby becomes ready within 2x lease_duration
#       and a new leader key appears carrying the standby's identity (spec §4.7)
#   (e) an in-place overwrite of the leader key (lease-detaching put, the
#       create_revision-preserving case) demotes the new leader within
#       ~renew_deadline, and it then re-campaigns and recovers (spec §4.4)
#
# etcd access uses the v3 REST gateway (POST /v3/kv/range, /v3/kv/put) so the
# script needs no etcdctl binary. Requires: jq, python3, curl, docker (only if
# etcd is not already reachable).
#
# Usage:
#   ./run_etcd_leader_test.sh                 # uses/starts the shared test etcd
#   ETCD_ENDPOINT=http://host:2379 ./run_etcd_leader_test.sh
# =============================================================================
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC}   $1"; }
log_error()   { echo -e "${RED}[ERR]${NC}  $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() { echo; echo -e "${BLUE}=== $1 ===${NC}"; }

command -v jq >/dev/null      || { log_error "jq is required"; exit 1; }
command -v python3 >/dev/null || { log_error "python3 is required"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../../.." && pwd)"
UTILS_DIR="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils"
ETCD_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/etcd"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
# shellcheck source=../../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

if [[ ! -x "$CONTROLLER_BIN" ]]; then
    log_info "Building edgion-controller (debug)"
    (cd "$PROJECT_ROOT" && cargo build --bin edgion-controller)
fi

# ---------------------------------------------------------------------------
# etcd: reuse a reachable endpoint or start the shared test container
# ---------------------------------------------------------------------------
ETCD_ENDPOINT="${ETCD_ENDPOINT:-http://127.0.0.1:12379}"
STARTED_ETCD=0
if ! curl -sf --max-time 3 "${ETCD_ENDPOINT}/health" > /dev/null 2>&1; then
    log_info "etcd not reachable at ${ETCD_ENDPOINT}; starting the shared test container"
    (cd "$ETCD_DIR" && docker compose up -d)
    STARTED_ETCD=1
    for _ in $(seq 1 30); do
        curl -sf --max-time 2 "${ETCD_ENDPOINT}/health" > /dev/null 2>&1 && break
        sleep 1
    done
    curl -sf --max-time 2 "${ETCD_ENDPOINT}/health" > /dev/null 2>&1 \
        || { log_error "etcd did not become healthy at ${ETCD_ENDPOINT}"; exit 1; }
fi
log_success "etcd healthy at ${ETCD_ENDPOINT}"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_ROOT="${PROJECT_ROOT}/tmp/integration_testing/etcd_leader_${TIMESTAMP}"
ETCD_PREFIX="/edgion-le-it-${TIMESTAMP}"
ELECTION_PREFIX="${ETCD_PREFIX}/leader"

# Election timing: fast failover for the test; floor(renew/retry) = 4 >= 3.
LEASE_SECS=6
RENEW_SECS=4
RETRY_SECS=1

# Advertised (never dialed) non-loopback addresses; validation rejects loopback.
A_ADMIN_PORT=15831; A_GRPC_PORT=15832; A_PROBE_PORT=15833; A_METRICS_PORT=15834
B_ADMIN_PORT=15841; B_GRPC_PORT=15842; B_PROBE_PORT=15843; B_METRICS_PORT=15844
A_ADV_ADMIN="10.255.0.1:${A_ADMIN_PORT}"
B_ADV_ADMIN="10.255.0.2:${B_ADMIN_PORT}"

export EDGION_ADMIN_TOKEN="etcd-le-test-token"

A_PID=""
B_PID=""
TEST_PASSED=0

cleanup() {
    log_section "Cleanup"
    for pid in "$A_PID" "$B_PID"; do
        if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
        fi
    done
    sleep 1
    for pid in "$A_PID" "$B_PID"; do
        [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null || true
    done
    if [[ "$STARTED_ETCD" -eq 1 ]]; then
        (cd "$ETCD_DIR" && docker compose down --timeout 5) 2>/dev/null || true
    fi
    if [[ "$TEST_PASSED" -eq 1 ]]; then
        rm -rf "$WORK_ROOT"
    else
        log_warn "Keeping WORK_ROOT for post-mortem: $WORK_ROOT"
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# etcd v3 REST helpers (no etcdctl needed)
# ---------------------------------------------------------------------------
b64() { printf '%s' "$1" | base64 | tr -d '\n'; }

# range_end = prefix with its last byte incremented (etcd prefix-scan idiom)
prefix_end_b64() {
    python3 - "$1" <<'PY'
import base64, sys
p = sys.argv[1].encode()
print(base64.b64encode(p[:-1] + bytes([p[-1] + 1])).decode())
PY
}

# All KVs under the election prefix, one JSON object per line:
# {key_b64, value_json, lease, create_revision}
election_kvs() {
    curl -s --max-time 5 -X POST "${ETCD_ENDPOINT}/v3/kv/range" \
        -d "{\"key\": \"$(b64 "$ELECTION_PREFIX")\", \"range_end\": \"$(prefix_end_b64 "$ELECTION_PREFIX")\"}" |
        jq -c '(.kvs // [])[] | {key_b64: .key, value_b64: .value, lease: (.lease // "0"), create_revision: (.create_revision // "0")}'
}

leader_key_count()  { election_kvs | wc -l | tr -d ' '; }
# v3election: the LEADER is the key with the LOWEST create_revision under the
# prefix; other lease-attached keys are queued candidates (their session leases
# are kept alive continuously, so they persist — that is correct behavior).
leader_admin_from_etcd() {
    election_kvs | jq -s -r 'sort_by(.create_revision | tonumber) | .[0].value_b64' | base64 -d | jq -r '.admin_addr'
}
# A successor has only truly won once the predecessor's key is gone (its lease
# expired). Print the admin_addr only when exactly one key remains — used after
# kill -9, when the dead process's key must vanish within one lease TTL.
sole_leader_admin() {
    local kvs
    kvs=$(election_kvs)
    [[ "$(printf '%s\n' "$kvs" | grep -c .)" == "1" ]] || return 0
    printf '%s' "$kvs" | jq -r '.value_b64' | base64 -d | jq -r '.admin_addr'
}

ready_code() { curl -s -o /dev/null -w '%{http_code}' --max-time 3 "http://127.0.0.1:$1/ready" || true; }

# ---------------------------------------------------------------------------
# Start two controllers
# ---------------------------------------------------------------------------
start_controller() { # $1=name  $2=admin  $3=grpc  $4=probe  $5=metrics  $6=adv_ip
    local name="$1" admin="$2" grpc="$3" probe="$4" metrics="$5" adv_ip="$6"
    local wd="${WORK_ROOT}/${name}"
    mkdir -p "${wd}/config" "${wd}/logs"
    ln -s "${PROJECT_ROOT}/config/crd" "${wd}/config/crd"
    # >&2: this function's stdout is the started PID; keep helper chatter off it.
    conf_sync_distribute_certs "$wd" >&2
    cat > "${wd}/config/edgion-controller.yaml" <<EOF
work_dir: "${wd}"

server:
  grpc_listen: "127.0.0.1:${grpc}"
  admin_listen: "127.0.0.1:${admin}"
  probe_listen: "127.0.0.1:${probe}"
  metrics_listen: "127.0.0.1:${metrics}"

logging:
  log_dir: "${wd}/logs"
  log_prefix: edgion-controller
  log_level: info
  console: false

conf_center:
  type: etcd
  endpoints: ["${ETCD_ENDPOINT}"]
  key_prefix: "${ETCD_PREFIX}"
  leader_election:
    identity: "${name}"
    grpc_addr: "${adv_ip}:${grpc}"
    admin_addr: "${adv_ip}:${admin}"
    lease_duration_secs: ${LEASE_SECS}
    renew_deadline_secs: ${RENEW_SECS}
    retry_period_secs: ${RETRY_SECS}

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
EOF
    conf_sync_security_block server >> "${wd}/config/edgion-controller.yaml"
    "$CONTROLLER_BIN" --config-file "${wd}/config/edgion-controller.yaml" \
        > "${wd}/logs/stdout.log" 2> "${wd}/logs/stderr.log" &
    echo $!
}

log_section "Start controller-a and controller-b (etcd mode, shared prefix ${ETCD_PREFIX})"
A_PID=$(start_controller controller-a "$A_ADMIN_PORT" "$A_GRPC_PORT" "$A_PROBE_PORT" "$A_METRICS_PORT" 10.255.0.1)
B_PID=$(start_controller controller-b "$B_ADMIN_PORT" "$B_GRPC_PORT" "$B_PROBE_PORT" "$B_METRICS_PORT" 10.255.0.2)
log_info "controller-a pid=${A_PID}, controller-b pid=${B_PID}"

for pid in "$A_PID" "$B_PID"; do
    sleep 0.2
    kill -0 "$pid" 2>/dev/null || {
        log_error "a controller exited immediately; logs under ${WORK_ROOT}"
        tail -40 "${WORK_ROOT}"/*/logs/stderr.log || true
        exit 1
    }
done

# ---------------------------------------------------------------------------
# (a) exactly one leader
# ---------------------------------------------------------------------------
log_section "(a) exactly one controller becomes ready (leader)"
LEADER=""
for _ in $(seq 1 40); do
    a=$(ready_code "$A_PROBE_PORT"); b=$(ready_code "$B_PROBE_PORT")
    if [[ "$a" == "200" && "$b" != "200" ]]; then LEADER="A"; break; fi
    if [[ "$b" == "200" && "$a" != "200" ]]; then LEADER="B"; break; fi
    if [[ "$a" == "200" && "$b" == "200" ]]; then
        log_error "BOTH controllers report ready — double leader"; exit 1
    fi
    sleep 1
done
[[ -n "$LEADER" ]] || { log_error "no controller became ready within 40s"; tail -30 "${WORK_ROOT}"/*/logs/stderr.log || true; exit 1; }
# Let the loser settle, then re-assert mutual exclusion.
sleep 2
a=$(ready_code "$A_PROBE_PORT"); b=$(ready_code "$B_PROBE_PORT")
[[ "$a" == "200" && "$b" == "200" ]] && { log_error "double leader after settle"; exit 1; }
log_success "exactly one leader: controller-$([[ "$LEADER" == "A" ]] && echo a || echo b) (/ready a=${a} b=${b})"

if [[ "$LEADER" == "A" ]]; then
    LEADER_PID=$A_PID;  LEADER_PROBE=$A_PROBE_PORT;  LEADER_ADV_ADMIN=$A_ADV_ADMIN
    FOLLOWER_ADMIN_PORT=$B_ADMIN_PORT; FOLLOWER_PROBE=$B_PROBE_PORT
    STANDBY_NAME="controller-b"; STANDBY_ADV_ADMIN=$B_ADV_ADMIN; STANDBY_METRICS=$B_METRICS_PORT
else
    LEADER_PID=$B_PID;  LEADER_PROBE=$B_PROBE_PORT;  LEADER_ADV_ADMIN=$B_ADV_ADMIN
    FOLLOWER_ADMIN_PORT=$A_ADMIN_PORT; FOLLOWER_PROBE=$A_PROBE_PORT
    STANDBY_NAME="controller-a"; STANDBY_ADV_ADMIN=$A_ADV_ADMIN; STANDBY_METRICS=$A_METRICS_PORT
fi

# ---------------------------------------------------------------------------
# (b) follower business routes 409 with the leader's admin_addr
# ---------------------------------------------------------------------------
log_section "(b) follower Admin API returns 409 naming the leader"
RESP_FILE="${WORK_ROOT}/follower_409.json"
# The follower learns the leader via its candidacy refresher (one retry_period
# cadence); allow a few seconds for the leader field to be populated.
B_OK=0
for _ in $(seq 1 10); do
    CODE=$(curl -s -o "$RESP_FILE" -w '%{http_code}' --max-time 5 \
        -H "Authorization: Bearer ${EDGION_ADMIN_TOKEN}" \
        "http://127.0.0.1:${FOLLOWER_ADMIN_PORT}/api/v1/cluster/GatewayClass")
    if [[ "$CODE" == "409" ]] \
        && [[ "$(jq -r '.error' "$RESP_FILE")" == "not leader" ]] \
        && [[ "$(jq -r '.leader' "$RESP_FILE")" == "$LEADER_ADV_ADMIN" ]]; then
        B_OK=1; break
    fi
    sleep 1
done
[[ "$B_OK" -eq 1 ]] || {
    log_error "follower did not 409 with leader=${LEADER_ADV_ADMIN}; last code=${CODE}, body:"
    cat "$RESP_FILE" || true
    exit 1
}
log_success "follower 409 body names the leader (${LEADER_ADV_ADMIN})"

# The follower's public probes must stay open (no 409 on the skip set).
[[ "$(ready_code "$FOLLOWER_PROBE")" == "503" ]] \
    || { log_error "follower /ready expected 503"; exit 1; }
log_success "follower /ready stays a plain 503 (public path not gated)"

# ---------------------------------------------------------------------------
# (c) election keys: leader (lowest create_revision) + queued candidate,
#     every key lease-attached
# ---------------------------------------------------------------------------
log_section "(c) election keys under ${ELECTION_PREFIX} (v3election semantics)"
# Both processes campaign with continuously-renewed session leases, so BOTH
# keys persist: the lowest create_revision is the leader, the other is the
# queued candidate. (Exactly-one-key would only hold if the candidate's lease
# were allowed to expire — the pre-review bug.)
COUNT=$(leader_key_count)
[[ "$COUNT" == "2" ]] || { log_error "expected leader + queued candidate = 2 keys, got ${COUNT}"; election_kvs; exit 1; }
LEASELESS=$(election_kvs | jq -r 'select(.lease == "0") | .key_b64' | wc -l | tr -d ' ')
[[ "$LEASELESS" == "0" ]] || { log_error "found ${LEASELESS} leaseless election key(s)"; election_kvs; exit 1; }
[[ "$(leader_admin_from_etcd)" == "$LEADER_ADV_ADMIN" ]] \
    || { log_error "lowest-create_revision key does not match the ready controller"; exit 1; }
log_success "2 lease-attached keys; lowest create_revision names controller-$([[ "$LEADER" == "A" ]] && echo a || echo b)"

# ---------------------------------------------------------------------------
# (d) kill -9 the leader; the standby must WIN THE ELECTION within
#     2x lease_duration (spec §4.7: the election is bounded by the lease), and
#     then become READY within the successor-startup budget. The two bounds are
#     deliberately separate: end-to-end readiness is dominated by application
#     startup — the Phase-1 init pump now runs inside spawn_all_controllers
#     before wait_kinds_ready(phase1) is called, so that gate resolves promptly
#     instead of always timing out; readiness is now dominated by election +
#     real application startup + the registry-ready wait — and is NOT bounded
#     by the lease TTL.
# ---------------------------------------------------------------------------
ELECTION_DEADLINE=$((LEASE_SECS * 2))
READY_DEADLINE=30
log_section "(d) kill -9 the leader; standby must win within ${ELECTION_DEADLINE}s and be ready within ${READY_DEADLINE}s"
kill -9 "$LEADER_PID"
if [[ "$LEADER" == "A" ]]; then A_PID=""; else B_PID=""; fi
T0=$(date +%s)

# (d1) election bound: a new leader key naming the standby, within 2x lease.
WON=""
while :; do
    ELAPSED=$(( $(date +%s) - T0 ))
    if [[ "$(sole_leader_admin 2>/dev/null || true)" == "$STANDBY_ADV_ADMIN" ]]; then
        WON=$ELAPSED; break
    fi
    [[ "$ELAPSED" -ge "$ELECTION_DEADLINE" ]] && break
    sleep 0.5
done
[[ -n "$WON" ]] || {
    log_error "standby did not win the election within ${ELECTION_DEADLINE}s of kill -9"
    election_kvs || true
    exit 1
}
log_success "standby ${STANDBY_NAME} won the election in ${WON}s (< ${ELECTION_DEADLINE}s)"

# (d2) readiness bound: /ready 200 within the successor-startup budget.
TOOK=""
while :; do
    ELAPSED=$(( $(date +%s) - T0 ))
    if [[ "$(ready_code "$FOLLOWER_PROBE")" == "200" ]]; then TOOK=$ELAPSED; break; fi
    [[ "$ELAPSED" -ge "$READY_DEADLINE" ]] && break
    sleep 0.5
done
[[ -n "$TOOK" ]] || { log_error "standby did not become ready within ${READY_DEADLINE}s of kill -9"; exit 1; }
log_success "standby ${STANDBY_NAME} ready in ${TOOK}s (< ${READY_DEADLINE}s; election ${WON}s + startup)"

# ---------------------------------------------------------------------------
# (e) in-place overwrite of the leader key demotes the leader (guard txn)
# ---------------------------------------------------------------------------
log_section "(e) lease-detaching overwrite of the leader key demotes the leader"
# Metrics before: transitions counted so far by the new leader.
trans_count() {
    curl -s --max-time 3 "http://127.0.0.1:${STANDBY_METRICS}/metrics" |
        awk '/^edgion_controller_leader_transitions_total/ {print $2; found=1} END {if (!found) print 0}'
}
BEFORE=$(trans_count)

LEADER_KEY_B64=$(election_kvs | jq -s -r 'sort_by(.create_revision | tonumber) | .[0].key_b64')
# A put WITHOUT a lease preserves create_revision but detaches the lease and
# changes the value — the exact case a create_revision-only guard would miss.
curl -s --max-time 5 -X POST "${ETCD_ENDPOINT}/v3/kv/put" \
    -d "{\"key\": \"${LEADER_KEY_B64}\", \"value\": \"$(b64 garbage)\"}" > /dev/null

# The guard txn runs on the retry cadence: expect a demotion (transitions +1)
# and then a re-campaign win (+1 more), ending ready again.
DEMOTED=0
for _ in $(seq 1 $((RENEW_SECS * 2 * 2))); do
    NOW_COUNT=$(trans_count)
    if [[ "${NOW_COUNT%%.*}" -ge $((${BEFORE%%.*} + 1)) ]]; then DEMOTED=1; break; fi
    sleep 0.5
done
[[ "$DEMOTED" -eq 1 ]] || { log_error "leader did not demote after the overwrite (guard txn missed it)"; exit 1; }
log_success "guard txn demoted the leader after the lease-detaching overwrite"

# Recovery: as the sole live candidate it must re-campaign and become ready.
RECOVERED=0
for _ in $(seq 1 60); do
    if [[ "$(ready_code "$FOLLOWER_PROBE")" == "200" ]]; then RECOVERED=1; break; fi
    sleep 1
done
[[ "$RECOVERED" -eq 1 ]] || { log_error "demoted leader did not recover leadership"; exit 1; }
AFTER=$(trans_count)
log_success "re-campaigned and recovered (transitions ${BEFORE} -> ${AFTER})"

log_section "All assertions passed"
TEST_PASSED=1
