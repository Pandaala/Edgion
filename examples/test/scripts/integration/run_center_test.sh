#!/usr/bin/env bash
# =============================================================================
# Center Integration Test Script
#
# Standalone test for the Center watch-sync pipeline + admin APIs.
# Starts 1 center + 2 controllers with different clusters and PluginMetaData,
# then validates watch sync, failover fan-out, and consistency detection.
#
# Port allocation (all on 127.0.0.1, avoids loopback alias):
#   center:       gRPC 50952, HTTP 5910
#   controller-1: gRPC 50953, admin 5911  (cluster=east-cluster)
#   controller-2: gRPC 50954, admin 5912  (cluster=west-cluster)
#
# Test cases:
#   1. watch_sync             — watch-status shows both controllers with sync_version > 0
#   2. metadata_store_cluster — metadata-store clusterRoutes has controllerCount >= 2
#   3. metadata_store_service — metadata-store serviceRoutes has >= 3 entries
#   4. failover_fanout        — POST failover, wait 2s, verify center + controller sides
#   5. consistency_detect     — consistency endpoint detects my_region conflict
#   6. svc_consistency_ok     — service-region-routes consistency is all consistent
#
# Usage:
#   ./run_center_test.sh             # Full run (build + test + cleanup)
#   ./run_center_test.sh --no-build  # Skip cargo build
# =============================================================================

set -euo pipefail

# ── Paths ────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
KILL_ALL="$REPO_ROOT/examples/test/scripts/utils/kill_all.sh"
CENTER_BIN="$REPO_ROOT/target/debug/edgion-center"
CTRL_BIN="$REPO_ROOT/target/debug/edgion-controller"
CONF_SRC="$REPO_ROOT/examples/test/conf/Center"

# Ports
CENTER_GRPC_PORT=50952
CENTER_HTTP_PORT=5910
CTRL1_GRPC_PORT=50953
CTRL1_ADMIN_PORT=5911
CTRL2_GRPC_PORT=50954
CTRL2_ADMIN_PORT=5912

CENTER_HTTP="http://127.0.0.1:${CENTER_HTTP_PORT}"
CTRL1_HTTP="http://127.0.0.1:${CTRL1_ADMIN_PORT}"
CTRL2_HTTP="http://127.0.0.1:${CTRL2_ADMIN_PORT}"

AUTH_USER="admin"
AUTH_PASS="test-center-pass"
JWT_SECRET="test-jwt-secret-center"

# ── State ────────────────────────────────────────────────────────────────────
WORK_DIR=""
CENTER_PID=""
CTRL1_PID=""
CTRL2_PID=""
TOKEN=""
PASS=0
FAIL=0

# ── Colours ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# ── Helpers ──────────────────────────────────────────────────────────────────
log()  { echo "[$(date '+%H:%M:%S')] $*"; }
pass() { echo -e "[$(date '+%H:%M:%S')] ${GREEN}PASS${NC}: $1"; ((PASS++)) || true; }
fail() { echo -e "[$(date '+%H:%M:%S')] ${RED}FAIL${NC}: $1 — $2"; ((FAIL++)) || true; }

cleanup() {
  log "Cleaning up..."
  # Use shared kill_all.sh to ensure no stale processes (covers center + controller + gateway)
  "$KILL_ALL" 2>/dev/null || true
  if [[ -n "$WORK_DIR" && -d "$WORK_DIR" ]]; then
    rm -rf "$WORK_DIR"
  fi
}
trap cleanup EXIT

# wait_for_http URL TIMEOUT_SECS
wait_for_http() {
  local url="$1"
  local timeout="$2"
  local elapsed=0
  log "Waiting for HTTP endpoint: $url (timeout ${timeout}s)"
  while ! curl -sf --max-time 2 "$url" >/dev/null 2>&1; do
    if [[ $elapsed -ge $timeout ]]; then
      echo -e "${RED}ERROR${NC}: Timed out waiting for $url" >&2
      exit 1
    fi
    sleep 1
    ((elapsed++)) || true
  done
  log "HTTP endpoint ready after ${elapsed}s"
}

# auth_get TOKEN URL — GET with Bearer token, returns response body
auth_get() {
  curl -sf --max-time 10 -H "Authorization: Bearer $1" "$2" 2>/dev/null || true
}

# auth_post TOKEN URL BODY — POST with Bearer token, returns response body
auth_post() {
  curl -sf --max-time 10 -H "Authorization: Bearer $1" -H "Content-Type: application/json" -d "$3" "$2" 2>/dev/null || true
}

# login URL USER PASS — returns JWT token
do_login() {
  local url="$1"
  local user="$2"
  local pass="$3"
  local resp
  resp=$(curl -sf --max-time 10 -H "Content-Type: application/json" \
    -d "{\"username\":\"${user}\",\"password\":\"${pass}\"}" \
    "${url}/api/v1/auth/login" 2>/dev/null || true)
  echo "$resp" | grep -o '"token":"[^"]*"' | head -1 | sed 's/"token":"//;s/"//'
}

# ── Parse args ────────────────────────────────────────────────────────────────
BUILD=true
for arg in "$@"; do
  [[ "$arg" == "--no-build" ]] && BUILD=false
done

# ── Build ─────────────────────────────────────────────────────────────────────
if $BUILD; then
  log "Building binaries..."
  (cd "$REPO_ROOT" && cargo build -p edgion --bin edgion-center --bin edgion-controller 2>&1 | tail -5)
  log "Build complete"
fi

for bin in "$CENTER_BIN" "$CTRL_BIN"; do
  if [[ ! -x "$bin" ]]; then
    echo -e "${RED}ERROR${NC}: Binary not found or not executable: $bin" >&2
    exit 1
  fi
done

# ── Kill stale processes ──────────────────────────────────────────────────────
log "Killing stale processes..."
"$KILL_ALL" 2>/dev/null || true

# ── Work dir ──────────────────────────────────────────────────────────────────
WORK_DIR=$(mktemp -d)
mkdir -p "$WORK_DIR/logs" "$WORK_DIR/ctrl1/conf" "$WORK_DIR/ctrl2/conf"
log "Work dir: $WORK_DIR"

# ── Write center config ───────────────────────────────────────────────────────
cat > "$WORK_DIR/center.yaml" <<EOF
server:
  grpc_addr: "0.0.0.0:${CENTER_GRPC_PORT}"
  http_addr: "0.0.0.0:${CENTER_HTTP_PORT}"

sync:
  list_interval_secs: 5
  list_timeout_secs: 10
  command_timeout_secs: 10
  ping_interval_secs: 5
  offline_evict_hours: 24

database:
  enabled: false

local_auth:
  enabled: true
  username: "${AUTH_USER}"
  password: "${AUTH_PASS}"
  jwt_secret: "${JWT_SECRET}"
  jwt_expiry_hours: 24
EOF

# ── Write controller configs ─────────────────────────────────────────────────
write_controller_config() {
  local idx="$1" grpc_port="$2" admin_port="$3" cluster="$4" ctrl_name="$5"
  local dir="$WORK_DIR/ctrl${idx}"
  sed "s|WORK_DIR_PLACEHOLDER|${dir}|g" > "${dir}/controller.yaml" <<EOF
work_dir: "WORK_DIR_PLACEHOLDER"

server:
  grpc_listen: "0.0.0.0:${grpc_port}"
  admin_listen: "0.0.0.0:${admin_port}"

logging:
  log_dir: "WORK_DIR_PLACEHOLDER/logs"
  log_prefix: "center-test-ctrl${idx}"
  log_level: "info"
  console: false

conf_center:
  type: "file_system"
  conf_dir: "WORK_DIR_PLACEHOLDER/conf"

conf_sync:
  no_sync_kinds: ["ReferenceGrant", "Secret"]

center:
  address: "http://127.0.0.1:${CENTER_GRPC_PORT}"
  name: "${ctrl_name}"
  cluster: "${cluster}"
  env: ["testing"]
  ping_interval_secs: 5
EOF
}

write_controller_config 1 "$CTRL1_GRPC_PORT" "$CTRL1_ADMIN_PORT" "east-cluster" "ctrl-east"
write_controller_config 2 "$CTRL2_GRPC_PORT" "$CTRL2_ADMIN_PORT" "west-cluster" "ctrl-west"

# ── Copy CRD schemas + test resources ────────────────────────────────────────
for idx in 1 2; do
  local_dir="$WORK_DIR/ctrl${idx}"
  mkdir -p "${local_dir}/config"
  cp -r "$REPO_ROOT/config/crd" "${local_dir}/config/"
  cp "$CONF_SRC/ctrl${idx}/"*.yaml "${local_dir}/conf/"
done
log "Copied CRD schemas and PluginMetaData resources to controller dirs"

# ── Start center ──────────────────────────────────────────────────────────────
log "Starting edgion-center..."
"$CENTER_BIN" -c "$WORK_DIR/center.yaml" > "$WORK_DIR/logs/center.log" 2>&1 &
CENTER_PID=$!
log "edgion-center PID: $CENTER_PID"
sleep 1
if ! kill -0 "$CENTER_PID" 2>/dev/null; then
  echo -e "${RED}ERROR${NC}: edgion-center exited immediately. Last log lines:" >&2
  tail -20 "$WORK_DIR/logs/center.log" >&2
  exit 1
fi
wait_for_http "$CENTER_HTTP/health" 15

# ── Login to center ──────────────────────────────────────────────────────────
log "Logging in to center..."
TOKEN=$(do_login "$CENTER_HTTP" "$AUTH_USER" "$AUTH_PASS")
if [[ -z "$TOKEN" ]]; then
  echo -e "${RED}ERROR${NC}: Failed to login to center" >&2
  tail -20 "$WORK_DIR/logs/center.log" >&2
  exit 1
fi
log "Login successful"

# ── Start controllers ────────────────────────────────────────────────────────
log "Starting edgion-controller 1 (east-cluster)..."
"$CTRL_BIN" -c "$WORK_DIR/ctrl1/controller.yaml" > "$WORK_DIR/logs/ctrl1.log" 2>&1 &
CTRL1_PID=$!
log "edgion-controller 1 PID: $CTRL1_PID"

log "Starting edgion-controller 2 (west-cluster)..."
"$CTRL_BIN" -c "$WORK_DIR/ctrl2/controller.yaml" > "$WORK_DIR/logs/ctrl2.log" 2>&1 &
CTRL2_PID=$!
log "edgion-controller 2 PID: $CTRL2_PID"

sleep 1
for i in 1 2; do
  pid_var="CTRL${i}_PID"
  if ! kill -0 "${!pid_var}" 2>/dev/null; then
    echo -e "${RED}ERROR${NC}: edgion-controller $i exited immediately. Last log lines:" >&2
    tail -20 "$WORK_DIR/logs/ctrl${i}.log" >&2
    exit 1
  fi
done

# ── Wait for sync ────────────────────────────────────────────────────────────
# Wait until both controllers appear in watch-status with sync_version > 0
log "Waiting for watch sync to complete (timeout 30s)..."
elapsed=0
while true; do
  out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/admin/watch-status")
  # Count controllers with syncVersion > 0
  synced=$(echo "$out" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    items = d.get('data', [])
    print(sum(1 for i in items if i.get('syncVersion', 0) > 0))
except:
    print(0)
" 2>/dev/null || echo "0")
  if [[ "$synced" -ge 2 ]]; then
    log "Both controllers synced after ${elapsed}s"
    break
  fi
  if [[ $elapsed -ge 30 ]]; then
    echo -e "${RED}ERROR${NC}: Timed out waiting for watch sync" >&2
    log "Last watch-status response: $out"
    exit 1
  fi
  sleep 1
  ((elapsed++)) || true
done

# ── Test cases ────────────────────────────────────────────────────────────────
log "Running test cases..."

# ─── Test 1: watch_sync ─────────────────────────────────────────────────────
out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/admin/watch-status")
if [[ -z "$out" ]]; then
  fail "watch_sync" "empty response from /admin/watch-status"
else
  count=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
items = d.get('data', [])
print(sum(1 for i in items if i.get('syncVersion', 0) > 0))
" 2>/dev/null || echo "0")
  if [[ "$count" -ge 2 ]]; then
    pass "watch_sync"
  else
    fail "watch_sync" "expected >= 2 controllers with syncVersion > 0, got $count. Response: $out"
  fi
fi

# ─── Test 2: metadata_store_cluster ─────────────────────────────────────────
out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/admin/metadata-store")
if [[ -z "$out" ]]; then
  fail "metadata_store_cluster" "empty response from /admin/metadata-store"
else
  has_entry=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
data = d.get('data', {})
routes = data.get('clusterRoutes', [])
print(sum(1 for r in routes if r.get('controllerCount', 0) >= 2))
" 2>/dev/null || echo "0")
  if [[ "$has_entry" -ge 1 ]]; then
    pass "metadata_store_cluster"
  else
    fail "metadata_store_cluster" "expected clusterRoutes with controllerCount >= 2. Response: $out"
  fi
fi

# ─── Test 3: metadata_store_service ─────────────────────────────────────────
# Verify serviceRoutes has >= 3 entries (svc-route-a, svc-route-b, svc-route-c)
if [[ -z "$out" ]]; then
  fail "metadata_store_service" "empty response (reused from previous)"
else
  svc_count=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
data = d.get('data', {})
routes = data.get('serviceRoutes', [])
print(len(routes))
" 2>/dev/null || echo "0")
  if [[ "$svc_count" -ge 3 ]]; then
    pass "metadata_store_service"
  else
    fail "metadata_store_service" "expected >= 3 serviceRoutes, got $svc_count. Response: $out"
  fi
fi

# ─── Test 4: failover_fanout ────────────────────────────────────────────────
# POST failover: set east region's failoverTo=west on ClusterRegionRoute
# Body uses camelCase to match PluginMetadataFailoverRequest serde format
failover_body='{"namespace":"default","name":"test-cluster-route","regionName":"east","failoverTo":"west"}'
out=$(auth_post "$TOKEN" "$CENTER_HTTP/api/v1/center/cluster-region-routes/failover" "$failover_body")
if [[ -z "$out" ]]; then
  fail "failover_fanout" "empty response from failover endpoint"
else
  modified=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
print(d.get('data', {}).get('modified', 0))
" 2>/dev/null || echo "0")
  if [[ "$modified" -ge 1 ]]; then
    pass "failover_fanout"
  else
    fail "failover_fanout" "expected modified >= 1, got $modified. Response: $out"
  fi
fi

# Wait for failover to propagate back to center via watch sync
log "Waiting 3s for failover to propagate..."
sleep 3

# ─── Test 4b: failover_verify_controller ────────────────────────────────────
# Check that controller-1's cluster-region-routes reflect the failover
# Controller uses auto-generated local_auth → we need to login.
# However, the proxy from center bypasses auth (uses base router).
# For direct check on controller, we query via center proxy instead.
ctrl1_id="east-cluster/ctrl-east"
ctrl1_id_encoded=$(echo "$ctrl1_id" | sed 's|/|~|g')
out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/proxy/${ctrl1_id_encoded}/api/v1/cluster-region-routes")
if [[ -z "$out" ]]; then
  fail "failover_verify_controller" "empty response from controller proxy"
else
  has_failover=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
items = d.get('data', [])
for item in items:
  if item.get('name') == 'test-cluster-route':
    regions = item.get('regions', [])
    if isinstance(regions, list):
      for r in regions:
        if isinstance(r, dict) and r.get('name') == 'east' and r.get('failoverTo') == 'west':
          print('found')
          sys.exit(0)
    # Also handle regions as JSON string
    elif isinstance(regions, str):
      import json as j2
      rlist = j2.loads(regions)
      for r in rlist:
        if r.get('name') == 'east' and r.get('failover_to') == 'west':
          print('found')
          sys.exit(0)
print('not_found')
" 2>/dev/null || echo "error")
  if [[ "$has_failover" == "found" ]]; then
    pass "failover_verify_controller"
  else
    fail "failover_verify_controller" "failoverTo not found on controller. Response: $out"
  fi
fi

# ─── Test 4c: failover_verify_center ────────────────────────────────────────
# Check that center's aggregated ClusterRegionRoutes reflect the failover
out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/cluster-region-routes")
if [[ -z "$out" ]]; then
  fail "failover_verify_center" "empty response from center cluster-region-routes"
else
  has_failover=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
items = d.get('data', [])
for item in items:
  if item.get('name') == 'test-cluster-route':
    controllers = item.get('controllers', {})
    for cid, entry in controllers.items():
      regions = entry.get('regions', [])
      for r in regions:
        if r.get('name') == 'east' and r.get('failoverTo') == 'west':
          print('found')
          sys.exit(0)
print('not_found')
" 2>/dev/null || echo "error")
  if [[ "$has_failover" == "found" ]]; then
    pass "failover_verify_center"
  else
    fail "failover_verify_center" "failoverTo not found on center side. Response: $out"
  fi
fi

# ─── Test 5: consistency_detect ─────────────────────────────────────────────
out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/cluster-region-routes/consistency")
if [[ -z "$out" ]]; then
  fail "consistency_detect" "empty response from consistency endpoint"
else
  has_conflict=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
reports = d.get('data', [])
for r in reports:
    if r.get('name') == 'test-cluster-route' and not r.get('consistent', True):
        conflicts = r.get('conflicts', [])
        for c in conflicts:
            if c.get('field') == 'my_region':
                print('found')
                sys.exit(0)
print('not_found')
" 2>/dev/null || echo "error")
  if [[ "$has_conflict" == "found" ]]; then
    pass "consistency_detect"
  else
    fail "consistency_detect" "expected my_region conflict for test-cluster-route. Response: $out"
  fi
fi

# ─── Test 6: svc_consistency_ok ─────────────────────────────────────────────
# ServiceRegionRoutes have no myRegion, so both controllers should be consistent
out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/service-region-routes/consistency")
if [[ -z "$out" ]]; then
  fail "svc_consistency_ok" "empty response from service consistency endpoint"
else
  all_consistent=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
reports = d.get('data', [])
if not reports:
    print('empty')
elif all(r.get('consistent', False) for r in reports):
    print('ok')
else:
    print('conflict')
" 2>/dev/null || echo "error")
  if [[ "$all_consistent" == "ok" ]]; then
    pass "svc_consistency_ok"
  else
    fail "svc_consistency_ok" "expected all service routes consistent. Response: $out"
  fi
fi

# ── Final report ──────────────────────────────────────────────────────────────
echo ""
log "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [[ $FAIL -eq 0 ]]; then
  echo -e "[$(date '+%H:%M:%S')] ${GREEN}Results: $PASS passed, $FAIL failed — ALL TESTS PASSED${NC}"
else
  echo -e "[$(date '+%H:%M:%S')] ${RED}Results: $PASS passed, $FAIL failed — SOME TESTS FAILED${NC}"
  log "Logs:"
  log "  Center:       $WORK_DIR/logs/center.log"
  log "  Controller 1: $WORK_DIR/logs/ctrl1.log"
  log "  Controller 2: $WORK_DIR/logs/ctrl2.log"
fi
log "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

[[ $FAIL -eq 0 ]] && exit 0 || exit 1
