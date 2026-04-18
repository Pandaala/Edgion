#!/usr/bin/env bash
# =============================================================================
# GlobalConnectionIpRestriction Center Integration Test Script
#
# Standalone test for the Center↔Controller fan-out pipeline for
# GlobalConnectionIpRestriction (GCIR) PluginMetaData.
# Starts 1 center + 2 controllers (empty of GCIR PMs), then creates,
# updates, patches, and deletes a GCIR resource via the Center API,
# verifying fan-out and watch-sync at each step.
#
# Port allocation (DIFFERENT from run_center_test.sh to allow sequential runs):
#   center:       gRPC 50962, HTTP 5920
#   controller-1: gRPC 50963, admin 5921  (cluster=cluster-a)
#   controller-2: gRPC 50964, admin 5922  (cluster=cluster-b)
#
# Test cases:
#   1. gcir_create_fanout       — POST creates GCIR, both controllers fanned-out
#   2. gcir_list_after_create   — GET list shows gcir-test with >= 2 controller entries
#   3. gcir_get_detail          — GET detail confirms data fields on both controllers
#   4. gcir_patch_enable        — PATCH /enable=false, verify controllers show enable==false
#   5. gcir_patch_active_profile — PUT second profile then PATCH /active-profile, verify switch
#   6. gcir_consistency_ok      — GET /consistency reports gcir-test as consistent
#   7. gcir_delete_fanout       — DELETE fan-out, verify resource disappears
#
# Usage:
#   ./run_gcir_center_test.sh             # Full run (build + test + cleanup)
#   ./run_gcir_center_test.sh --no-build  # Skip cargo build
# =============================================================================

set -euo pipefail

# ── Paths ────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
KILL_ALL="$REPO_ROOT/examples/test/scripts/utils/kill_all.sh"
CENTER_BIN="$REPO_ROOT/target/debug/edgion-center"
CTRL_BIN="$REPO_ROOT/target/debug/edgion-controller"

# Ports — distinct from run_center_test.sh (50952/5910/50953/5911/50954/5912)
CENTER_GRPC_PORT=50962
CENTER_HTTP_PORT=5920
CTRL1_GRPC_PORT=50963
CTRL1_ADMIN_PORT=5921
CTRL2_GRPC_PORT=50964
CTRL2_ADMIN_PORT=5922

CENTER_HTTP="http://127.0.0.1:${CENTER_HTTP_PORT}"

AUTH_USER="admin"
AUTH_PASS="test-gcir-pass"
JWT_SECRET="test-jwt-secret-gcir"

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

# auth_post TOKEN URL BODY — POST with Bearer token + JSON, returns response body
auth_post() {
  curl -sf --max-time 10 \
    -H "Authorization: Bearer $1" \
    -H "Content-Type: application/json" \
    -d "$3" "$2" 2>/dev/null || true
}

# auth_put TOKEN URL BODY — PUT with Bearer token + JSON, returns response body
auth_put() {
  curl -sf --max-time 10 \
    -X PUT \
    -H "Authorization: Bearer $1" \
    -H "Content-Type: application/json" \
    -d "$3" "$2" 2>/dev/null || true
}

# auth_patch TOKEN URL BODY — PATCH with Bearer token + JSON, returns response body
auth_patch() {
  curl -sf --max-time 10 \
    -X PATCH \
    -H "Authorization: Bearer $1" \
    -H "Content-Type: application/json" \
    -d "$3" "$2" 2>/dev/null || true
}

# auth_delete TOKEN URL BODY — DELETE with Bearer token + JSON body, returns response body
auth_delete() {
  curl -sf --max-time 10 \
    -X DELETE \
    -H "Authorization: Bearer $1" \
    -H "Content-Type: application/json" \
    -d "$3" "$2" 2>/dev/null || true
}

# do_login URL USER PASS — returns JWT token
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
  cat > "${dir}/controller.yaml" <<EOF
work_dir: "${dir}"

server:
  grpc_listen: "0.0.0.0:${grpc_port}"
  admin_listen: "0.0.0.0:${admin_port}"

logging:
  log_dir: "${dir}/logs"
  log_prefix: "gcir-test-ctrl${idx}"
  log_level: "info"
  console: false

conf_center:
  type: "file_system"
  conf_dir: "${dir}/conf"

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

write_controller_config 1 "$CTRL1_GRPC_PORT" "$CTRL1_ADMIN_PORT" "cluster-a" "ctrl-a"
write_controller_config 2 "$CTRL2_GRPC_PORT" "$CTRL2_ADMIN_PORT" "cluster-b" "ctrl-b"

# ── Copy CRD schemas ONLY (no test resource YAMLs — controllers start empty of GCIR PMs) ──
for idx in 1 2; do
  local_dir="$WORK_DIR/ctrl${idx}"
  mkdir -p "${local_dir}/config"
  cp -r "$REPO_ROOT/config/crd" "${local_dir}/config/"
done
log "Copied CRD schemas to controller dirs (no pre-existing GCIR PMs)"

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
log "Starting edgion-controller 1 (cluster-a)..."
"$CTRL_BIN" -c "$WORK_DIR/ctrl1/controller.yaml" > "$WORK_DIR/logs/ctrl1.log" 2>&1 &
CTRL1_PID=$!
log "edgion-controller 1 PID: $CTRL1_PID"

log "Starting edgion-controller 2 (cluster-b)..."
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

# ── Wait for both controllers to connect to center ────────────────────────────
log "Waiting for both controllers to connect to center (timeout 30s)..."
elapsed=0
while true; do
  out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/admin/watch-status")
  synced=$(echo "$out" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    items = d.get('data', [])
    print(len(items))
except:
    print(0)
" 2>/dev/null || echo "0")
  if [[ "$synced" -ge 2 ]]; then
    log "Both controllers connected after ${elapsed}s"
    break
  fi
  if [[ $elapsed -ge 30 ]]; then
    echo -e "${RED}ERROR${NC}: Timed out waiting for controllers to connect" >&2
    log "Last watch-status response: $out"
    exit 1
  fi
  sleep 1
  ((elapsed++)) || true
done

# ── Test cases ────────────────────────────────────────────────────────────────
log "Running test cases..."

# ─── Test 1: gcir_create_fanout ──────────────────────────────────────────────
# POST a new GCIR via Center; it should fan-out to both controllers.
# Response shape: ApiResponse<FanOutResponse>
#   { success: true, data: { success: [...], failed: [...], warnings: [...] } }
CREATE_BODY='{
  "namespace": "edgion-test",
  "name": "gcir-test",
  "controllers": ["all"],
  "data": {
    "enable": true,
    "activeProfile": "strict",
    "profiles": {
      "strict": {
        "defaultAction": "deny",
        "allow": [{"name": "office", "cidrs": ["192.168.1.0/24"]}]
      }
    }
  }
}'
out=$(auth_post "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions" "$CREATE_BODY")
if [[ -z "$out" ]]; then
  fail "gcir_create_fanout" "empty response from POST /global-connection-ip-restrictions"
else
  result=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
fanout = d.get('data', {})
s = len(fanout.get('success', []))
f = len(fanout.get('failed', []))
if s >= 2 and f == 0:
    print('ok')
else:
    print(f'success={s} failed={f}')
" 2>/dev/null || echo "error")
  if [[ "$result" == "ok" ]]; then
    pass "gcir_create_fanout"
  else
    fail "gcir_create_fanout" "expected success>=2 failed=0, got $result. Response: $out"
  fi
fi

# Wait for watch-sync to propagate the new PM back to center's metadata store
log "Waiting 2s for watch-sync to propagate create..."
sleep 2

# ─── Test 2: gcir_list_after_create ──────────────────────────────────────────
# GET /global-connection-ip-restrictions — list shows gcir-test with >= 2 controller entries.
# Response shape: ApiResponse<Vec<CenterGlobalIpRestrictionView>>
#   { success: true, data: [ { namespace, name, controllers: {...}, ... } ] }
out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions")
if [[ -z "$out" ]]; then
  fail "gcir_list_after_create" "empty response from GET /global-connection-ip-restrictions"
else
  result=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
items = d.get('data', [])
for item in items:
    if item.get('name') == 'gcir-test':
        ctrls = item.get('controllers', {})
        n = len(ctrls)
        if n >= 2:
            print('ok')
        else:
            print(f'only {n} controllers')
        sys.exit(0)
print('not_found')
" 2>/dev/null || echo "error")
  if [[ "$result" == "ok" ]]; then
    pass "gcir_list_after_create"
  else
    fail "gcir_list_after_create" "gcir-test not found or not enough controllers: $result. Response: $out"
  fi
fi

# ─── Test 3: gcir_get_detail ─────────────────────────────────────────────────
# GET /global-connection-ip-restrictions/edgion-test/gcir-test
# Verify: name, namespace, controllers >= 2, each has enable/activeProfile/profiles.strict.defaultAction
out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions/edgion-test/gcir-test")
if [[ -z "$out" ]]; then
  fail "gcir_get_detail" "empty response from GET /global-connection-ip-restrictions/edgion-test/gcir-test"
else
  result=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
item = d.get('data', {})
if item.get('name') != 'gcir-test':
    print(f'wrong name: {item.get(\"name\")}')
    sys.exit(0)
if item.get('namespace') != 'edgion-test':
    print(f'wrong namespace: {item.get(\"namespace\")}')
    sys.exit(0)
ctrls = item.get('controllers', {})
if len(ctrls) < 2:
    print(f'only {len(ctrls)} controllers')
    sys.exit(0)
# Check at least one controller entry has expected fields
for cid, entry in ctrls.items():
    if not entry.get('enable', False):
        print(f'ctrl {cid} enable not true')
        sys.exit(0)
    if entry.get('activeProfile') != 'strict':
        print(f'ctrl {cid} activeProfile={entry.get(\"activeProfile\")}')
        sys.exit(0)
    profiles = entry.get('profiles', {})
    strict = profiles.get('strict', {})
    if strict.get('defaultAction') != 'deny':
        print(f'ctrl {cid} strict.defaultAction={strict.get(\"defaultAction\")}')
        sys.exit(0)
print('ok')
" 2>/dev/null || echo "error")
  if [[ "$result" == "ok" ]]; then
    pass "gcir_get_detail"
  else
    fail "gcir_get_detail" "$result. Response: $out"
  fi
fi

# ─── Test 4: gcir_patch_enable ───────────────────────────────────────────────
# PATCH /enable with enable=false; then GET detail and verify all controllers show enable==false.
PATCH_ENABLE_BODY='{"enable": false, "controllers": ["all"]}'
out=$(auth_patch "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions/edgion-test/gcir-test/enable" "$PATCH_ENABLE_BODY")
if [[ -z "$out" ]]; then
  fail "gcir_patch_enable" "empty response from PATCH /enable"
else
  fanout_ok=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
fanout = d.get('data', {})
f = len(fanout.get('failed', []))
print('ok' if f == 0 else f'failed={f}')
" 2>/dev/null || echo "error")
  if [[ "$fanout_ok" != "ok" ]]; then
    fail "gcir_patch_enable" "fan-out had failures: $fanout_ok. Response: $out"
  else
    log "Waiting 2s for PATCH enable=false to propagate..."
    sleep 2
    # Verify via GET detail
    out2=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions/edgion-test/gcir-test")
    result=$(echo "$out2" | python3 -c "
import sys, json
d = json.load(sys.stdin)
item = d.get('data', {})
ctrls = item.get('controllers', {})
if len(ctrls) < 2:
    print(f'only {len(ctrls)} controllers')
    sys.exit(0)
for cid, entry in ctrls.items():
    if entry.get('enable', True):
        print(f'ctrl {cid} still enabled')
        sys.exit(0)
print('ok')
" 2>/dev/null || echo "error")
    if [[ "$result" == "ok" ]]; then
      pass "gcir_patch_enable"
    else
      fail "gcir_patch_enable" "after patch, not all controllers have enable=false: $result. Response: $out2"
    fi
  fi
fi

# ─── Test 5: gcir_patch_active_profile ───────────────────────────────────────
# Step 1: re-enable and add a second profile via PUT.
# Step 2: PATCH /active-profile to switch to "permissive".
# Step 3: GET detail and verify all controllers have activeProfile == "permissive".

# Re-enable and add "permissive" profile via PUT
PUT_BODY='{
  "controllers": ["all"],
  "data": {
    "enable": true,
    "activeProfile": "strict",
    "profiles": {
      "strict": {
        "defaultAction": "deny",
        "allow": [{"name": "office", "cidrs": ["192.168.1.0/24"]}]
      },
      "permissive": {
        "defaultAction": "allow",
        "deny": [{"name": "bad", "cidrs": ["1.2.3.4/32"]}]
      }
    }
  }
}'
out=$(auth_put "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions/edgion-test/gcir-test" "$PUT_BODY")
put_ok=$(echo "$out" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    fanout = d.get('data', {})
    f = len(fanout.get('failed', []))
    print('ok' if f == 0 else f'failed={f}')
except:
    print('parse_error')
" 2>/dev/null || echo "error")
if [[ "$put_ok" != "ok" ]]; then
  fail "gcir_patch_active_profile" "PUT to add permissive profile failed: $put_ok. Response: $out"
else
  log "Waiting 2s for PUT to propagate..."
  sleep 2

  # Now PATCH active-profile to "permissive"
  PATCH_PROFILE_BODY='{"activeProfile": "permissive", "controllers": ["all"]}'
  out=$(auth_patch "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions/edgion-test/gcir-test/active-profile" "$PATCH_PROFILE_BODY")
  patch_ok=$(echo "$out" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    fanout = d.get('data', {})
    f = len(fanout.get('failed', []))
    print('ok' if f == 0 else f'failed={f}')
except:
    print('parse_error')
" 2>/dev/null || echo "error")
  if [[ "$patch_ok" != "ok" ]]; then
    fail "gcir_patch_active_profile" "PATCH /active-profile failed: $patch_ok. Response: $out"
  else
    log "Waiting 2s for active-profile patch to propagate..."
    sleep 2

    # Verify via GET detail
    out2=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions/edgion-test/gcir-test")
    result=$(echo "$out2" | python3 -c "
import sys, json
d = json.load(sys.stdin)
item = d.get('data', {})
ctrls = item.get('controllers', {})
if len(ctrls) < 2:
    print(f'only {len(ctrls)} controllers')
    sys.exit(0)
for cid, entry in ctrls.items():
    ap = entry.get('activeProfile')
    if ap != 'permissive':
        print(f'ctrl {cid} activeProfile={ap}')
        sys.exit(0)
print('ok')
" 2>/dev/null || echo "error")
    if [[ "$result" == "ok" ]]; then
      pass "gcir_patch_active_profile"
    else
      fail "gcir_patch_active_profile" "not all controllers switched to permissive: $result. Response: $out2"
    fi
  fi
fi

# ─── Test 6: gcir_consistency_ok ─────────────────────────────────────────────
# GET /global-connection-ip-restrictions/consistency
# Assert gcir-test entry has consistent == true (all controllers fanned out the same data).
out=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions/consistency")
if [[ -z "$out" ]]; then
  fail "gcir_consistency_ok" "empty response from GET /consistency"
else
  result=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
reports = d.get('data', [])
for r in reports:
    if r.get('name') == 'gcir-test':
        if r.get('consistent', False):
            print('ok')
        else:
            conflicts = r.get('conflicts', [])
            print(f'not consistent, conflicts={conflicts}')
        sys.exit(0)
print('not_found')
" 2>/dev/null || echo "error")
  if [[ "$result" == "ok" ]]; then
    pass "gcir_consistency_ok"
  else
    fail "gcir_consistency_ok" "gcir-test not consistent: $result. Response: $out"
  fi
fi

# ─── Test 7: gcir_delete_fanout ──────────────────────────────────────────────
# DELETE /edgion-test/gcir-test with body {"controllers": ["all"]}
# Assert failed.length == 0, then GET list to verify gcir-test is gone.
DELETE_BODY='{"controllers": ["all"]}'
out=$(auth_delete "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions/edgion-test/gcir-test" "$DELETE_BODY")
if [[ -z "$out" ]]; then
  fail "gcir_delete_fanout" "empty response from DELETE"
else
  fanout_ok=$(echo "$out" | python3 -c "
import sys, json
d = json.load(sys.stdin)
fanout = d.get('data', {})
f = len(fanout.get('failed', []))
print('ok' if f == 0 else f'failed={f}')
" 2>/dev/null || echo "error")
  if [[ "$fanout_ok" != "ok" ]]; then
    fail "gcir_delete_fanout" "delete fan-out had failures: $fanout_ok. Response: $out"
  else
    log "Waiting 2s for delete to propagate via watch-sync..."
    sleep 2
    # Verify gcir-test is no longer in list
    out2=$(auth_get "$TOKEN" "$CENTER_HTTP/api/v1/center/global-connection-ip-restrictions")
    result=$(echo "$out2" | python3 -c "
import sys, json
d = json.load(sys.stdin)
items = d.get('data', [])
for item in items:
    if item.get('name') == 'gcir-test':
        print('still_present')
        sys.exit(0)
print('ok')
" 2>/dev/null || echo "error")
    if [[ "$result" == "ok" ]]; then
      pass "gcir_delete_fanout"
    else
      fail "gcir_delete_fanout" "gcir-test still present in list after delete. Response: $out2"
    fi
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
