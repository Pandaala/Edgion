#!/usr/bin/env bash
# =============================================================================
# FedSync Integration Test Script
#
# Standalone test for the controller <-> center federation sync.
# Starts edgion-center and edgion-controller, waits for registration,
# then validates sync behavior via the center HTTP Admin API.
#
# Test cases:
#   1. controller_registered   — controller appears in /controllers
#   2. cluster_visible         — cluster name appears in /clusters
#   3. resources_synced        — HTTPRoute and Gateway synced to /resources
#   4. no_sync_kinds_filtered  — Secret not present in /resources?kind=Secret
#   5. reload_command          — POST /controllers/{id}/reload returns ok
#
# Usage:
#   ./run_fed_sync_test.sh             # Full run (build + test + cleanup)
#   ./run_fed_sync_test.sh --no-build  # Skip cargo build
# =============================================================================

set -euo pipefail

# ── Paths ────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CENTER_BIN="$REPO_ROOT/target/debug/edgion-center"
CTRL_BIN="$REPO_ROOT/target/debug/edgion-controller"
CENTER_HTTP="http://127.0.0.1:5910"
CONTROLLER_ID="test-cluster/test-ctrl"
CONTROLLER_ID_ENCODED="test-cluster%2Ftest-ctrl"
CONF_SRC="$REPO_ROOT/examples/test/conf/fed_sync"

# ── State ────────────────────────────────────────────────────────────────────
WORK_DIR=""
CENTER_PID=""
CTRL_PID=""
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
  if [[ -n "$CTRL_PID" ]]; then
    kill "$CTRL_PID" 2>/dev/null || true
  fi
  if [[ -n "$CENTER_PID" ]]; then
    kill "$CENTER_PID" 2>/dev/null || true
  fi
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

# wait_for_registration TIMEOUT_SECS
wait_for_registration() {
  local timeout="$1"
  local elapsed=0
  log "Waiting for controller '$CONTROLLER_ID' to register (timeout ${timeout}s)"
  while true; do
    local out
    out=$(curl -sf --max-time 5 "$CENTER_HTTP/controllers" 2>/dev/null || true)
    if echo "$out" | grep -q '"controller_id":"'"$CONTROLLER_ID"'"'; then
      log "Controller registered after ${elapsed}s"
      return 0
    fi
    if [[ $elapsed -ge $timeout ]]; then
      echo -e "${RED}ERROR${NC}: Timed out waiting for controller registration" >&2
      log "Last /controllers response: $out"
      exit 1
    fi
    sleep 1
    ((elapsed++)) || true
  done
}

# wait_for_resources TIMEOUT_SECS
wait_for_resources() {
  local timeout="$1"
  local elapsed=0
  log "Waiting for resources to sync (timeout ${timeout}s)"
  while true; do
    local out
    out=$(curl -sf --max-time 5 "$CENTER_HTTP/resources?cluster=test-cluster" 2>/dev/null || true)
    if [[ -n "$out" && "$out" != "[]" && "$out" != "null" ]]; then
      log "Resources synced after ${elapsed}s"
      return 0
    fi
    if [[ $elapsed -ge $timeout ]]; then
      echo -e "${RED}ERROR${NC}: Timed out waiting for resources to sync" >&2
      log "Last /resources response: $out"
      exit 1
    fi
    sleep 1
    ((elapsed++)) || true
  done
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

# Verify binaries exist
for bin in "$CENTER_BIN" "$CTRL_BIN"; do
  if [[ ! -x "$bin" ]]; then
    echo -e "${RED}ERROR${NC}: Binary not found or not executable: $bin" >&2
    exit 1
  fi
done

# ── Work dir ──────────────────────────────────────────────────────────────────
WORK_DIR=$(mktemp -d)
mkdir -p "$WORK_DIR/logs" "$WORK_DIR/conf"
log "Work dir: $WORK_DIR"

# ── Write center config ───────────────────────────────────────────────────────
cat > "$WORK_DIR/center.yaml" <<'EOF'
server:
  grpc_addr: "0.0.0.0:50952"
  http_addr: "0.0.0.0:5910"

sync:
  list_interval_secs: 5
  list_timeout_secs: 10
  command_timeout_secs: 10
  ping_interval_secs: 5
EOF

# ── Write controller config (substitute WORK_DIR) ─────────────────────────────
sed "s|WORK_DIR_PLACEHOLDER|${WORK_DIR}|g" > "$WORK_DIR/controller.yaml" <<EOF
work_dir: "WORK_DIR_PLACEHOLDER"

server:
  grpc_listen: "0.0.0.0:50953"
  admin_listen: "0.0.0.0:5911"

logging:
  log_dir: "WORK_DIR_PLACEHOLDER/logs"
  log_prefix: "fed-sync-test-ctrl"
  log_level: "info"
  json_format: false
  console: false

conf_center:
  type: "file_system"
  conf_dir: "WORK_DIR_PLACEHOLDER/conf"

conf_sync:
  no_sync_kinds: ["ReferenceGrant", "Secret"]

center:
  address: "http://127.0.0.1:50952"
  name: "test-ctrl"
  cluster: "test-cluster"
  env: ["testing"]
  ping_interval_secs: 5
EOF

# ── Copy test resources ───────────────────────────────────────────────────────
cp "$CONF_SRC"/*.yaml "$WORK_DIR/conf/"
log "Copied $(ls "$WORK_DIR/conf/"*.yaml 2>/dev/null | wc -l | tr -d ' ') YAML resources to conf dir"

# ── Start center ──────────────────────────────────────────────────────────────
log "Starting edgion-center..."
"$CENTER_BIN" -c "$WORK_DIR/center.yaml" > "$WORK_DIR/center.log" 2>&1 &
CENTER_PID=$!
log "edgion-center PID: $CENTER_PID"
sleep 1
if ! kill -0 "$CENTER_PID" 2>/dev/null; then
  echo -e "${RED}ERROR${NC}: edgion-center exited immediately. Last log lines:" >&2
  tail -20 "$WORK_DIR/center.log" >&2
  exit 1
fi

wait_for_http "$CENTER_HTTP/controllers" 15

# ── Start controller ──────────────────────────────────────────────────────────
log "Starting edgion-controller..."
"$CTRL_BIN" -c "$WORK_DIR/controller.yaml" > "$WORK_DIR/controller.log" 2>&1 &
CTRL_PID=$!
log "edgion-controller PID: $CTRL_PID"
sleep 1
if ! kill -0 "$CTRL_PID" 2>/dev/null; then
  echo -e "${RED}ERROR${NC}: edgion-controller exited immediately. Last log lines:" >&2
  tail -20 "$WORK_DIR/controller.log" >&2
  exit 1
fi

wait_for_registration 30
wait_for_resources 30

# ── Test cases ────────────────────────────────────────────────────────────────
log "Running test cases..."

# Test 1: controller_registered
out=$(curl -sf --max-time 5 "$CENTER_HTTP/controllers" 2>/dev/null || true)
if [[ -z "$out" ]]; then
  fail "controller_registered" "curl failed or returned empty response from /controllers"
elif echo "$out" | grep -q '"controller_id":"'"$CONTROLLER_ID"'"'; then
  pass "controller_registered"
else
  fail "controller_registered" "controller_id '$CONTROLLER_ID' not found in /controllers response: $out"
fi

# Test 2: cluster_visible
out=$(curl -sf --max-time 5 "$CENTER_HTTP/clusters" 2>/dev/null || true)
if [[ -z "$out" ]]; then
  fail "cluster_visible" "curl failed or returned empty response from /clusters"
elif echo "$out" | grep -q "test-cluster"; then
  pass "cluster_visible"
else
  fail "cluster_visible" "cluster 'test-cluster' not found in /clusters response: $out"
fi

# Test 3: resources_synced
out=$(curl -sf --max-time 5 "$CENTER_HTTP/resources?cluster=test-cluster" 2>/dev/null || true)
if [[ -z "$out" ]]; then
  fail "resources_synced" "curl failed or returned empty response from /resources"
elif echo "$out" | grep -q "HTTPRoute" && echo "$out" | grep -q "Gateway"; then
  pass "resources_synced"
else
  fail "resources_synced" "HTTPRoute and/or Gateway not found in /resources response: $out"
fi

# Test 4: no_sync_kinds_filtered
out=$(curl -sf --max-time 5 "$CENTER_HTTP/resources?cluster=test-cluster&kind=Secret" 2>/dev/null || true)
if [[ "$out" == "[]" || -z "$out" ]]; then
  pass "no_sync_kinds_filtered"
else
  fail "no_sync_kinds_filtered" "expected empty array for Secret kind, got: $out"
fi

# Test 5: reload_command
body_file=$(mktemp)
http_code=$(curl -s -w "%{http_code}" -o "$body_file" --max-time 10 -X POST \
  "$CENTER_HTTP/controllers/${CONTROLLER_ID_ENCODED}/reload" 2>/dev/null || echo "000")
out=$(cat "$body_file" 2>/dev/null || true); rm -f "$body_file"
if [[ "$http_code" == "000" ]]; then
  fail "reload_command" "curl failed (network error or timeout)"
elif [[ "$http_code" == "200" ]] && [[ "$out" == *'"ok":true'* ]]; then
  pass "reload_command"
else
  fail "reload_command" "expected HTTP 200 with {\"ok\":true}, got HTTP $http_code: $out"
fi

# ── Final report ──────────────────────────────────────────────────────────────
echo ""
log "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [[ $FAIL -eq 0 ]]; then
  echo -e "[$(date '+%H:%M:%S')] ${GREEN}Results: $PASS passed, $FAIL failed — ALL TESTS PASSED${NC}"
else
  echo -e "[$(date '+%H:%M:%S')] ${RED}Results: $PASS passed, $FAIL failed — SOME TESTS FAILED${NC}"
  log "Center log: $WORK_DIR/center.log"
  log "Controller log: $WORK_DIR/controller.log"
fi
log "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

[[ $FAIL -eq 0 ]] && exit 0 || exit 1
