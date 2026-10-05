#!/usr/bin/env bash
# =============================================================================
# run_etcd_leader_failover.sh — End-to-end etcd HA failover (Plan B Task 8;
# spec §10 integration tests 1/2/4/6 — the whole-feature acceptance criterion).
#
# Topology: 1 etcd + 2 controllers (etcd mode, shared prefix, shared conf_sync
# mTLS cert set) + 1 gateway with `server_addrs` listing BOTH controllers.
#
# Asserts:
#   (1) bootstrap: the gateway becomes ready connected to whichever controller
#       is leader — the rotator's per-gateway shuffle means the follower is
#       first in the list on ~half the runs (spec §7.3);
#   (2) a config change pushed via the leader's Admin API reaches the gateway;
#   (3) `kill -9` the leader → the standby takes over and a NEW config change
#       pushed via it reaches the gateway (spec §10 test 4). Budget: election
#       (2x lease) + successor startup (Phase-1 stall + registry-ready) + one
#       gateway rotation lap;
#   (4) a controller RELOAD on the live leader does NOT rotate the gateway away
#       (spec §10 test 6, §7.4 H2) — asserted by a config change landing after
#       the reload AND by the gateway log showing no new rotation events.
#
# SAN note (spec §7.6): both controllers share one cert set and both addresses
# are 127.0.0.1 (different ports), so the localhost SAN covers both. A real
# multi-host deployment must issue the server cert with a SAN per controller
# address (`edgion-cli gen-certs --server-san`, repeatable) — see
# docs/en/ops-guide/infrastructure/mtls.md.
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

command -v jq >/dev/null || { log_error "jq is required"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../../.." && pwd)"
UTILS_DIR="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils"
ETCD_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/etcd"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
GATEWAY_BIN="${PROJECT_ROOT}/target/debug/edgion-gateway"
EDGION_CLI="${PROJECT_ROOT}/target/debug/edgion-cli"
# shellcheck source=../../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

for bin in "$CONTROLLER_BIN" "$GATEWAY_BIN" "$EDGION_CLI"; do
    if [[ ! -x "$bin" ]]; then
        log_info "Building $(basename "$bin") (debug)"
        (cd "$PROJECT_ROOT" && cargo build --bin "$(basename "$bin")")
    fi
done

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
        || { log_error "etcd did not become healthy"; exit 1; }
fi
log_success "etcd healthy at ${ETCD_ENDPOINT}"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_ROOT="${PROJECT_ROOT}/tmp/integration_testing/etcd_failover_${TIMESTAMP}"
ETCD_PREFIX="/edgion-le-e2e-${TIMESTAMP}"

LEASE_SECS=6
RENEW_SECS=4
RETRY_SECS=1

A_ADMIN_PORT=15931; A_GRPC_PORT=15932; A_PROBE_PORT=15933; A_METRICS_PORT=15934
B_ADMIN_PORT=15941; B_GRPC_PORT=15942; B_PROBE_PORT=15943; B_METRICS_PORT=15944
GW_PROBE_PORT=15950; GW_ADMIN_PORT=15951; GW_PROXY_PORT=31941

export EDGION_ADMIN_TOKEN="etcd-le-e2e-token"
export EDGION_GATEWAY_ADMIN_TOKEN="etcd-le-e2e-gw-token"
export EDGION_TOKEN="$EDGION_ADMIN_TOKEN" # edgion-cli auth

A_PID=""; B_PID=""; GW_PID=""
TEST_PASSED=0

cleanup() {
    log_section "Cleanup"
    for pid in "$GW_PID" "$A_PID" "$B_PID"; do
        [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
    done
    sleep 1
    for pid in "$GW_PID" "$A_PID" "$B_PID"; do
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

ready_code() { curl -s -o /dev/null -w '%{http_code}' --max-time 3 "http://127.0.0.1:$1/ready" || true; }

gw_api() { # $1=path
    curl -s --max-time 5 -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" \
        "http://127.0.0.1:${GW_ADMIN_PORT}$1" 2>/dev/null || echo '{"success":false}'
}

apply_via() { # $1=admin_port $2=file
    "$EDGION_CLI" --server "http://127.0.0.1:$1" apply -f "$2"
}

# ---------------------------------------------------------------------------
# Work dirs, shared certs, configs
# ---------------------------------------------------------------------------
log_section "Preparing work dirs (shared conf_sync cert set)"
mkdir -p "${WORK_ROOT}"/{controller-a,controller-b,gateway}/{config,logs}
ln -s "${PROJECT_ROOT}/config/crd" "${WORK_ROOT}/controller-a/config/crd"
ln -s "${PROJECT_ROOT}/config/crd" "${WORK_ROOT}/controller-b/config/crd"

# ONE cert set for the whole topology: the gateway holds one client cert + CA,
# so both controllers must present server certs from the same CA.
conf_sync_distribute_certs "${WORK_ROOT}/controller-a"
conf_sync_copy_certs "${WORK_ROOT}/controller-a" "${WORK_ROOT}/controller-b"
conf_sync_copy_certs "${WORK_ROOT}/controller-a" "${WORK_ROOT}/gateway"

write_controller_config() { # $1=name $2=admin $3=grpc $4=probe $5=metrics $6=adv_ip
    local name="$1" admin="$2" grpc="$3" probe="$4" metrics="$5" adv_ip="$6"
    local wd="${WORK_ROOT}/${name}"
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
  controller_name: edgion.io/gateway-controller
  endpoints: ["${ETCD_ENDPOINT}"]
  key_prefix: "${ETCD_PREFIX}"
  request_timeout_ms: 1000
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
}

write_controller_config controller-a "$A_ADMIN_PORT" "$A_GRPC_PORT" "$A_PROBE_PORT" "$A_METRICS_PORT" 10.255.0.1
write_controller_config controller-b "$B_ADMIN_PORT" "$B_GRPC_PORT" "$B_PROBE_PORT" "$B_METRICS_PORT" 10.255.0.2

GW_WD="${WORK_ROOT}/gateway"
cat > "${GW_WD}/config/edgion-gateway.yaml" <<EOF
gateway:
  server_addrs: ["https://127.0.0.1:${A_GRPC_PORT}", "https://127.0.0.1:${B_GRPC_PORT}"]
  admin_listen: "127.0.0.1:${GW_ADMIN_PORT}"
  probe_listen: "127.0.0.1:${GW_PROBE_PORT}"

logging:
  log_dir: "${GW_WD}/logs"
  log_prefix: edgion-gateway
  log_level: info
  console: false

ssl_log:
  enabled: false
EOF
conf_sync_security_block client >> "${GW_WD}/config/edgion-gateway.yaml"

# Test fixtures: a minimal Gateway topology + HTTPRoute probes.
FIXTURES="${WORK_ROOT}/fixtures"
mkdir -p "$FIXTURES"
cp "${PROJECT_ROOT}/edgion-tests/integration/conf/base/GatewayClass.yaml" "$FIXTURES/"
cp "${PROJECT_ROOT}/edgion-tests/integration/conf/base/EdgionGatewayConfig.yaml" "$FIXTURES/"
cat > "${FIXTURES}/Gateway.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: le-gateway
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: http
      protocol: HTTP
      port: ${GW_PROXY_PORT}
EOF
route_yaml() { # $1=name
    cat <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: $1
  namespace: edgion-test
spec:
  parentRefs:
    - name: le-gateway
  hostnames:
    - "$1.edgion-test.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: le-backend
          port: 8080
EOF
}
route_yaml route-one   > "${FIXTURES}/route-one.yaml"
route_yaml route-two   > "${FIXTURES}/route-two.yaml"
route_yaml route-three > "${FIXTURES}/route-three.yaml"

# ---------------------------------------------------------------------------
# Start controllers; find the leader
# ---------------------------------------------------------------------------
log_section "Start controller-a and controller-b (etcd mode)"
"$CONTROLLER_BIN" --config-file "${WORK_ROOT}/controller-a/config/edgion-controller.yaml" \
    > "${WORK_ROOT}/controller-a/logs/stdout.log" 2> "${WORK_ROOT}/controller-a/logs/stderr.log" &
A_PID=$!
"$CONTROLLER_BIN" --config-file "${WORK_ROOT}/controller-b/config/edgion-controller.yaml" \
    > "${WORK_ROOT}/controller-b/logs/stdout.log" 2> "${WORK_ROOT}/controller-b/logs/stderr.log" &
B_PID=$!

LEADER=""
for _ in $(seq 1 40); do
    a=$(ready_code "$A_PROBE_PORT"); b=$(ready_code "$B_PROBE_PORT")
    if [[ "$a" == "200" && "$b" != "200" ]]; then LEADER="A"; break; fi
    if [[ "$b" == "200" && "$a" != "200" ]]; then LEADER="B"; break; fi
    if [[ "$a" == "200" && "$b" == "200" ]]; then log_error "double leader"; exit 1; fi
    sleep 1
done
[[ -n "$LEADER" ]] || { log_error "no leader within 40s"; tail -20 "${WORK_ROOT}"/controller-*/logs/stderr.log; exit 1; }
if [[ "$LEADER" == "A" ]]; then
    LEADER_PID=$A_PID; LEADER_ADMIN=$A_ADMIN_PORT; LEADER_PROBE=$A_PROBE_PORT
    STANDBY_ADMIN=$B_ADMIN_PORT; STANDBY_PROBE=$B_PROBE_PORT; STANDBY_NAME="controller-b"
else
    LEADER_PID=$B_PID; LEADER_ADMIN=$B_ADMIN_PORT; LEADER_PROBE=$B_PROBE_PORT
    STANDBY_ADMIN=$A_ADMIN_PORT; STANDBY_PROBE=$A_PROBE_PORT; STANDBY_NAME="controller-a"
fi
log_success "leader is controller-$([[ "$LEADER" == "A" ]] && echo a || echo b)"

log_section "Apply base topology via the leader"
apply_via "$LEADER_ADMIN" "${FIXTURES}/EdgionGatewayConfig.yaml" > /dev/null
apply_via "$LEADER_ADMIN" "${FIXTURES}/GatewayClass.yaml" > /dev/null
apply_via "$LEADER_ADMIN" "${FIXTURES}/Gateway.yaml" > /dev/null
log_success "GatewayClass + EdgionGatewayConfig + Gateway applied"

# ---------------------------------------------------------------------------
# (1) Gateway bootstrap finds the leader (shuffled server_addrs)
# ---------------------------------------------------------------------------
log_section "(1) gateway bootstrap lands on the leader"
EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
"$GATEWAY_BIN" -c "${GW_WD}/config/edgion-gateway.yaml" --work-dir "$GW_WD" --integration-testing-mode \
    > "${GW_WD}/logs/stdout.log" 2> "${GW_WD}/logs/stderr.log" &
GW_PID=$!

GW_READY=0
for _ in $(seq 1 60); do
    if [[ "$(ready_code "$GW_PROBE_PORT")" == "200" ]]; then GW_READY=1; break; fi
    kill -0 "$GW_PID" 2>/dev/null || { log_error "gateway exited during bootstrap"; tail -30 "${GW_WD}"/logs/*.log; exit 1; }
    sleep 1
done
[[ "$GW_READY" -eq 1 ]] || { log_error "gateway not ready within 60s"; tail -30 "${GW_WD}"/logs/*.log; exit 1; }
RESP=$(gw_api "/configclient/Gateway?namespace=edgion-test&name=le-gateway")
[[ "$(echo "$RESP" | jq -r '.success')" == "true" ]] \
    || { log_error "gateway did not sync the Gateway resource: $RESP"; exit 1; }
log_success "gateway bootstrapped onto the leader and synced the base topology"

# ---------------------------------------------------------------------------
# (2) config change via the leader reaches the gateway
# ---------------------------------------------------------------------------
log_section "(2) config change via leader reaches the gateway"
apply_via "$LEADER_ADMIN" "${FIXTURES}/route-one.yaml" > /dev/null
SYNCED=0
for _ in $(seq 1 20); do
    RESP=$(gw_api "/configclient/HTTPRoute?namespace=edgion-test&name=route-one")
    [[ "$(echo "$RESP" | jq -r '.success')" == "true" ]] && { SYNCED=1; break; }
    sleep 1
done
[[ "$SYNCED" -eq 1 ]] || { log_error "route-one did not reach the gateway"; exit 1; }
log_success "route-one synced to the gateway"

# ---------------------------------------------------------------------------
# (3) kill -9 the leader: gateway follows the new leader (ACCEPTANCE)
# ---------------------------------------------------------------------------
# Budget: election (<= 2x lease = 12s) + successor startup (Phase-1 15s stall +
# registry-ready, <= 30s) + gateway rotation/backoff laps.
FAILOVER_BUDGET=75
log_section "(3) kill -9 the leader; gateway must follow the new leader (budget ${FAILOVER_BUDGET}s)"
kill -9 "$LEADER_PID"
if [[ "$LEADER" == "A" ]]; then A_PID=""; else B_PID=""; fi
T0=$(date +%s)

NEW_READY=""
while :; do
    ELAPSED=$(( $(date +%s) - T0 ))
    if [[ "$(ready_code "$STANDBY_PROBE")" == "200" ]]; then NEW_READY=$ELAPSED; break; fi
    [[ "$ELAPSED" -ge 50 ]] && break
    sleep 1
done
[[ -n "$NEW_READY" ]] || { log_error "standby ${STANDBY_NAME} not ready within 50s of kill -9"; exit 1; }
log_info "standby ${STANDBY_NAME} ready ${NEW_READY}s after kill -9"

apply_via "$STANDBY_ADMIN" "${FIXTURES}/route-two.yaml" > /dev/null
SYNCED=0
while :; do
    ELAPSED=$(( $(date +%s) - T0 ))
    RESP=$(gw_api "/configclient/HTTPRoute?namespace=edgion-test&name=route-two")
    [[ "$(echo "$RESP" | jq -r '.success')" == "true" ]] && { SYNCED=1; break; }
    [[ "$ELAPSED" -ge "$FAILOVER_BUDGET" ]] && break
    sleep 1
done
[[ "$SYNCED" -eq 1 ]] || {
    log_error "route-two (pushed via the NEW leader) did not reach the gateway within ${FAILOVER_BUDGET}s"
    tail -30 "${GW_WD}"/logs/stdout.log
    exit 1
}
TOOK=$(( $(date +%s) - T0 ))
log_success "gateway followed the new leader and received route-two ${TOOK}s after kill -9 (< ${FAILOVER_BUDGET}s)"

# ---------------------------------------------------------------------------
# (4) a controller reload must NOT rotate the gateway away
# ---------------------------------------------------------------------------
log_section "(4) leader reload does not rotate the gateway away"
# Channel installation cancels the old generation and wakes the resource
# watchers. Controller reload only sends switch_event on the same channel, so
# these installation events must not increase. Keep the nonzero failover guard.
# Tracing output goes to the file logger (console: false), not stdout.
gw_installations() { cat "${GW_WD}"/logs/edgion-gateway*.log 2>/dev/null | grep -c "channel reinstalled" || true; }

# Settle first: (3) proves config sync via the per-kind loops, but the meta
# coordinator may still be finishing its own failover laps. Measuring the
# reload against a still-rotating baseline would attribute failover tail
# notifications to the reload. Wait until their count is stable for 10s.
SETTLED=0
STABLE_SINCE=$(date +%s)
LAST_COUNT=$(gw_installations)
for _ in $(seq 1 60); do
    sleep 2
    NOW_COUNT=$(gw_installations)
    if [[ "$NOW_COUNT" != "$LAST_COUNT" ]]; then
        LAST_COUNT=$NOW_COUNT
        STABLE_SINCE=$(date +%s)
    elif [[ $(( $(date +%s) - STABLE_SINCE )) -ge 10 ]]; then
        SETTLED=1
        break
    fi
done
[[ "$SETTLED" -eq 1 ]] || { log_error "gateway channel installation events never settled after the failover (count still moving: ${LAST_COUNT})"; exit 1; }
log_info "coordinator settled (${LAST_COUNT} channel installation notifications during failover)"

INSTALLATIONS_BEFORE=$(gw_installations)
# Sanity: the failover in (3) must have produced installation notifications —
# if this is 0 the log path is wrong and the (4) assertion would be vacuous.
[[ "${INSTALLATIONS_BEFORE:-0}" -ge 1 ]] \
    || { log_error "expected >=1 channel installation notification during the failover, found ${INSTALLATIONS_BEFORE} (log path wrong?)"; exit 1; }
RELOAD_CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 -X POST \
    -H "Authorization: Bearer ${EDGION_ADMIN_TOKEN}" "http://127.0.0.1:${STANDBY_ADMIN}/api/v1/reload")
[[ "$RELOAD_CODE" == "200" || "$RELOAD_CODE" == "202" ]] \
    || { log_error "reload on the live leader returned ${RELOAD_CODE}"; exit 1; }

# Wait for the leader to come back (reload rebuilds the CSS; /ready may blip).
RECOVERED=0
for _ in $(seq 1 45); do
    if [[ "$(ready_code "$STANDBY_PROBE")" == "200" ]]; then RECOVERED=1; break; fi
    sleep 1
done
[[ "$RECOVERED" -eq 1 ]] || { log_error "leader did not become ready again after reload"; exit 1; }

apply_via "$STANDBY_ADMIN" "${FIXTURES}/route-three.yaml" > /dev/null
SYNCED=0
for _ in $(seq 1 30); do
    RESP=$(gw_api "/configclient/HTTPRoute?namespace=edgion-test&name=route-three")
    [[ "$(echo "$RESP" | jq -r '.success')" == "true" ]] && { SYNCED=1; break; }
    sleep 1
done
[[ "$SYNCED" -eq 1 ]] || { log_error "route-three did not reach the gateway after the reload"; exit 1; }

INSTALLATIONS_AFTER=$(gw_installations)
[[ "${INSTALLATIONS_AFTER:-0}" -eq "${INSTALLATIONS_BEFORE:-0}" ]] || {
    log_error "the gateway rotated during a reload (before=${INSTALLATIONS_BEFORE} after=${INSTALLATIONS_AFTER}) — reload must be a same-address reconnect (spec §7.4 H2)"
    exit 1
}
# The dead ex-leader must still be dead-and-not-leader (no flap back).
[[ "$(ready_code "$LEADER_PROBE")" != "200" ]] || { log_error "killed ex-leader reports ready"; exit 1; }
log_success "reload stayed on the same controller (installation notifications ${INSTALLATIONS_BEFORE:-0} -> ${INSTALLATIONS_AFTER:-0}); route-three synced"

log_section "All assertions passed"
TEST_PASSED=1
