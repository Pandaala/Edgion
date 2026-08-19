#!/bin/bash
# =============================================================================
# Unknown Config Fields Integration Test
#
# The suite guard in run_integration.sh (verify_no_unknown_config_fields)
# proves ABSENCE: it fails the run if any resource in the fleet carries a
# typo'd config key. This script proves PRESENCE end to end:
#
#   1. apply an EdgionPlugins fixture with `config.denys` (typo for `deny`);
#      assert the diagnostics endpoint reports exactly
#      `requestPlugins[0].config.denys`, that the resource is still
#      `Accepted: True` (warn-not-reject, decision D-4 / 05-issues.md R-12),
#      and that an `UnknownConfigFields` status condition is present;
#   2. apply a ForwardAuth fixture with `successStatusCodes` -- a key that no
#      longer exists, AND one `deny_unknown_fields` structurally cannot catch
#      because ForwardAuthConfig `#[serde(flatten)]`s its connection struct;
#      assert it is reported;
#   3. apply an EdgionStreamPlugins fixture with the same `denys` typo;
#      assert it is reported;
#   4. overwrite all three with corrected YAML; assert the diagnostics
#      endpoint drains to `total: 0` AND the `UnknownConfigFields` condition
#      is REMOVED from each resource's status (proves the upsert-and-remove
#      discipline, not just upsert);
#   5. delete the resources; assert no ghost entries remain;
#   6. exercise `POST /api/v1/validate/{kind}` once: a typo'd document
#      returns `ok: false` with the exact path, and nothing is persisted by
#      that call (the resource must not appear in the store afterwards).
#
# Uses the FileSystem backend, no gateway, no K8s -- modeled on
# test_resource_abnormal_metrics.sh.
#
# Fixtures live under edgion-tests/integration/fixtures/unknown-config-fields/,
# deliberately OUTSIDE edgion-tests/integration/conf/. That placement is
# load-bearing: run_integration.sh loads the whole conf tree into the
# controller, so a deliberately-malformed fixture parked there would be applied
# as real config and would trip the suite's own unknown-config-fields guard
# before Round 1 (it did, once). Negative fixtures for a scenario that starts
# its own controller with its own conf dir must not live in the shared corpus.
# =============================================================================
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
UTILS_DIR="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils"
FIXTURE_DIR="${PROJECT_ROOT}/edgion-tests/integration/fixtures/unknown-config-fields"
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/unknown_config_fields_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
CONF_WATCH_DIR="${WORK_DIR}/conf"
LOG_DIR="${WORK_DIR}/logs"
PID_DIR="${WORK_DIR}/pids"

CONTROLLER_ADMIN_PORT=58700
CONTROLLER_GRPC_PORT=58701
CONTROLLER_PROBE_PORT=58702
CONTROLLER_METRICS_PORT=58703
ADMIN_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"

# Fixed test-only admin token, not a real credential (same convention as
# run_integration.sh's main-suite token).
ADMIN_TOKEN="integration-unknown-fields-admin-token-0123456789"

# Namespace/names used across every fixture in FIXTURE_DIR.
NS="edgion-test"
IPR_NAME="uf-ipr"
FA_NAME="uf-fa"
SP_NAME="uf-sp"

CONTROLLER_PID=""
TEST_PASSED=0

# --- Log helpers ---
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC}   $1"; }
log_error()   { echo -e "${RED}[ERR]${NC}  $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# --- Cleanup ---
cleanup() {
    log_section "Cleanup"
    if [[ -n "$CONTROLLER_PID" ]] && kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_info "Stopping controller PID=$CONTROLLER_PID"
        kill "$CONTROLLER_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$CONTROLLER_PID" 2>/dev/null || true
    fi
    if [[ "$TEST_PASSED" -eq 1 ]]; then
        log_info "Removing WORK_DIR (test passed): $WORK_DIR"
        rm -rf "$WORK_DIR"
    else
        log_warn "Keeping WORK_DIR for post-mortem: $WORK_DIR"
    fi
}
trap cleanup EXIT

# --- Setup ---
log_section "Setup"
mkdir -p "$CONFIG_DIR" "$CONF_WATCH_DIR" "$LOG_DIR" "$PID_DIR"
log_info "WORK_DIR=$WORK_DIR"

# Make CRD schemas reachable from work_dir (non-K8s mode requires config/crd).
ln -s "${PROJECT_ROOT}/config/crd" "${CONFIG_DIR}/crd"

# conf_sync is mTLS by default: put the certs where the emitted block expects
# them (work-dir relative), then let the shared emitter write the transport
# block.
conf_sync_distribute_certs "$WORK_DIR"

cat > "${CONFIG_DIR}/edgion-controller.yaml" <<EOF
work_dir: "${WORK_DIR}"

server:
  grpc_listen: "127.0.0.1:${CONTROLLER_GRPC_PORT}"
  admin_listen: "127.0.0.1:${CONTROLLER_ADMIN_PORT}"
  probe_listen: "127.0.0.1:${CONTROLLER_PROBE_PORT}"
  metrics_listen: "127.0.0.1:${CONTROLLER_METRICS_PORT}"

logging:
  log_dir: "${LOG_DIR}"
  log_prefix: edgion-controller
  log_level: info
  json_format: false
  console: false
  buffer_size: 10000

conf_center:
  type: file_system
  conf_dir: "${CONF_WATCH_DIR}"

debug:
  enabled: false

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
EOF
conf_sync_security_block server >> "${CONFIG_DIR}/edgion-controller.yaml"
log_info "Config file written: ${CONFIG_DIR}/edgion-controller.yaml"

# --- Launch controller ---
log_section "Start Controller"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
if [[ ! -x "$CONTROLLER_BIN" ]]; then
    log_error "Controller binary not found at $CONTROLLER_BIN"
    log_info "Build with: cargo build --bin edgion-controller"
    exit 1
fi

EDGION_ADMIN_TOKEN="$ADMIN_TOKEN" \
"$CONTROLLER_BIN" \
    --config-file "${CONFIG_DIR}/edgion-controller.yaml" \
    > "${LOG_DIR}/controller.stdout.log" 2> "${LOG_DIR}/controller.stderr.log" &
CONTROLLER_PID=$!
echo "$CONTROLLER_PID" > "${PID_DIR}/controller.pid"
log_info "Controller started PID=$CONTROLLER_PID"

log_info "Waiting for controller readiness..."
READY=0
for i in $(seq 1 30); do
    if curl -sf "http://127.0.0.1:${CONTROLLER_PROBE_PORT}/health" > /dev/null 2>&1; then
        READY=1
        break
    fi
    if ! kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_error "Controller process exited during startup"
        log_info "=== controller.stderr.log (last 40 lines) ==="
        tail -40 "${LOG_DIR}/controller.stderr.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
done
if [[ "$READY" -ne 1 ]]; then
    log_error "Controller did not become ready in 30s"
    tail -40 "${LOG_DIR}/controller.stderr.log" 2>/dev/null || true
    exit 1
fi
log_success "Controller ready on port ${CONTROLLER_ADMIN_PORT}"

# --- Diagnostics-endpoint helpers ---

# Fetch and dump the diagnostics endpoint as "TOTAL=<n>" followed by
# "<kind>/<namespace>/<name> -> <path1>|<path2>|..." lines (one per item,
# `|`-joined paths so a caller can grep a single item's line safely).
diag_snapshot() {
    curl -sf -m 10 -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${ADMIN_URL}/api/v1/diagnostics/unknown-fields" 2>/dev/null | python3 -c '
import json, sys
try:
    doc = json.load(sys.stdin)
except Exception:
    sys.exit(1)
data = doc.get("data") or {}
total = data.get("total", 0)
print(f"TOTAL={total}")
for item in data.get("items", []):
    kind = item.get("kind", "?")
    ns = item.get("namespace", "")
    name = item.get("name", "?")
    paths = "|".join(item.get("paths", []))
    print(f"{kind}/{ns}/{name} -> {paths}")
'
}

diag_total() {
    diag_snapshot | head -1 | sed 's/^TOTAL=//'
}

# Wait until diag_snapshot's total matches the expected value.
wait_for_diag_total() {
    local expected="$1" label="$2"
    local timeout=30 elapsed=0
    while (( elapsed < timeout )); do
        local total
        total=$(diag_total 2>/dev/null || echo "")
        if [ "$total" = "$expected" ]; then
            log_success "$label: total=${expected} after ${elapsed}s"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    log_error "$label: TIMEOUT (${timeout}s); expected total=${expected}, got total=${total:-<unreachable>}"
    diag_snapshot || true
    return 1
}

# Assert one item's line contains the given kind/namespace/name and every
# given path substring.
assert_item_reported() {
    local kind="$1" ns="$2" name="$3"; shift 3
    local snapshot line
    snapshot=$(diag_snapshot)
    line=$(echo "$snapshot" | grep -F "${kind}/${ns}/${name} ->" || true)
    if [ -z "$line" ]; then
        log_error "expected ${kind}/${ns}/${name} in diagnostics, got:"
        echo "$snapshot"
        return 1
    fi
    local path
    for path in "$@"; do
        if ! echo "$line" | grep -qF -- "$path"; then
            log_error "expected path '${path}' in line: ${line}"
            return 1
        fi
    done
    log_success "diagnostics reports ${kind}/${ns}/${name} with expected path(s): $*"
}

# Print the status of a named condition ("True"/"False"), "ABSENT" if the
# condition is not present, or "MISSING" if the file does not exist. Parses
# the file-mode `.status` sidecar without PyYAML (serde emits fields in
# struct order, `status:` immediately before `type:` within each condition
# list item -- same idiom as test_plugin_policy.sh's condition_status).
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

# Wait until the given .status file's named condition reaches want_status
# ("True"/"False"/"ABSENT"/"MISSING").
wait_condition_status() {
    local file="$1" cond="$2" want_status="$3" label="$4"
    local timeout=30 elapsed=0
    while (( elapsed < timeout )); do
        local st
        st=$(condition_status "$file" "$cond")
        if [ "$st" = "$want_status" ]; then
            log_success "$label: ${cond}=${want_status} after ${elapsed}s"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    log_error "$label: TIMEOUT (${timeout}s); expected ${cond}=${want_status}, got $(condition_status "$file" "$cond")"
    return 1
}

status_file_for() {
    local kind="$1" ns="$2" name="$3"
    echo "${CONF_WATCH_DIR}/${kind}_${ns}_${name}.yaml.status"
}

# --- Test sequence ---

log_section "Test 1: EdgionPlugins IpRestriction — config.denys reported, Accepted stays True"
cp "${FIXTURE_DIR}/EdgionPlugins_edgion-test_uf-ipr.yaml" "${CONF_WATCH_DIR}/"
wait_for_diag_total 1 "after uf-ipr apply"
assert_item_reported "EdgionPlugins" "$NS" "$IPR_NAME" "requestPlugins[0].config.denys"
IPR_STATUS=$(status_file_for EdgionPlugins "$NS" "$IPR_NAME")
wait_condition_status "$IPR_STATUS" "UnknownConfigFields" "True" "uf-ipr UnknownConfigFields present"
acc=$(condition_status "$IPR_STATUS" "Accepted")
if [ "$acc" != "True" ]; then
    log_error "uf-ipr Accepted=${acc}, expected True (warn-not-reject must not affect Accepted)"
    exit 1
fi
log_success "uf-ipr Accepted=True (warn-not-reject holds)"

log_section "Test 2: EdgionPlugins ForwardAuth — flattened successStatusCodes reported"
cp "${FIXTURE_DIR}/EdgionPlugins_edgion-test_uf-fa.yaml" "${CONF_WATCH_DIR}/"
wait_for_diag_total 2 "after uf-fa apply"
assert_item_reported "EdgionPlugins" "$NS" "$FA_NAME" "requestPlugins[0].config.successStatusCodes"
FA_STATUS=$(status_file_for EdgionPlugins "$NS" "$FA_NAME")
wait_condition_status "$FA_STATUS" "UnknownConfigFields" "True" "uf-fa UnknownConfigFields present"

log_section "Test 3: EdgionStreamPlugins — plugins[0].config.denys reported"
cp "${FIXTURE_DIR}/EdgionStreamPlugins_edgion-test_uf-sp.yaml" "${CONF_WATCH_DIR}/"
wait_for_diag_total 3 "after uf-sp apply"
assert_item_reported "EdgionStreamPlugins" "$NS" "$SP_NAME" "plugins[0].config.denys"
SP_STATUS=$(status_file_for EdgionStreamPlugins "$NS" "$SP_NAME")
wait_condition_status "$SP_STATUS" "UnknownConfigFields" "True" "uf-sp UnknownConfigFields present"

log_section "Test 4: corrected YAML drains diagnostics to zero and removes the condition"
cp "${FIXTURE_DIR}/EdgionPlugins_edgion-test_uf-ipr.fixed.yaml" "${CONF_WATCH_DIR}/EdgionPlugins_${NS}_${IPR_NAME}.yaml"
cp "${FIXTURE_DIR}/EdgionPlugins_edgion-test_uf-fa.fixed.yaml" "${CONF_WATCH_DIR}/EdgionPlugins_${NS}_${FA_NAME}.yaml"
cp "${FIXTURE_DIR}/EdgionStreamPlugins_edgion-test_uf-sp.fixed.yaml" "${CONF_WATCH_DIR}/EdgionStreamPlugins_${NS}_${SP_NAME}.yaml"
wait_for_diag_total 0 "after all three fixtures fixed"
wait_condition_status "$IPR_STATUS" "UnknownConfigFields" "ABSENT" "uf-ipr UnknownConfigFields removed"
wait_condition_status "$FA_STATUS" "UnknownConfigFields" "ABSENT" "uf-fa UnknownConfigFields removed"
wait_condition_status "$SP_STATUS" "UnknownConfigFields" "ABSENT" "uf-sp UnknownConfigFields removed"

log_section "Test 5: delete resources — no ghost entries"
rm -f "${CONF_WATCH_DIR}/EdgionPlugins_${NS}_${IPR_NAME}.yaml"
rm -f "${CONF_WATCH_DIR}/EdgionPlugins_${NS}_${FA_NAME}.yaml"
rm -f "${CONF_WATCH_DIR}/EdgionStreamPlugins_${NS}_${SP_NAME}.yaml"
wait_for_diag_total 0 "after delete (no ghosts)"
snapshot_after_delete=$(diag_snapshot)
if [ "$(echo "$snapshot_after_delete" | wc -l | tr -d ' ')" != "1" ]; then
    log_error "expected only the TOTAL=0 line after delete, got:"
    echo "$snapshot_after_delete"
    exit 1
fi
log_success "no ghost entries after delete"

log_section "Test 6: POST /api/v1/validate/{kind} reports without persisting"
VALIDATE_BODY='{
  "apiVersion": "edgion.io/v1",
  "kind": "EdgionPlugins",
  "metadata": {"name": "uf-validate-only", "namespace": "'"$NS"'"},
  "spec": {
    "requestPlugins": [
      {"enable": true, "type": "IpRestriction", "config": {"defaultAction": "allow", "denys": []}}
    ]
  }
}'
validate_resp=$(curl -sf -m 10 -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    -H "Content-Type: application/json" \
    -X POST --data "$VALIDATE_BODY" \
    "${ADMIN_URL}/api/v1/validate/EdgionPlugins" 2>/dev/null)
if [ -z "$validate_resp" ]; then
    log_error "POST /api/v1/validate/EdgionPlugins did not respond"
    exit 1
fi
echo "$validate_resp" | python3 -c '
import json, sys
doc = json.load(sys.stdin)
data = doc.get("data") or {}
ok = data.get("ok")
fields = data.get("unknownFields") or []
assert ok is False, f"expected ok=False, got {ok!r}"
assert fields == ["requestPlugins[0].config.denys"], f"expected exact path, got {fields!r}"
print("OK: validate reported ok=False with exact path", fields)
'
# Nothing must have been persisted by the validate call: the raw store must
# not contain uf-validate-only, and no file must have appeared in the
# watched conf dir.
if find "${CONF_WATCH_DIR}" -iname "*uf-validate-only*" | grep -q .; then
    log_error "validate call persisted a file to the conf dir"
    find "${CONF_WATCH_DIR}" -iname "*uf-validate-only*"
    exit 1
fi
final_total=$(diag_total)
if [ "$final_total" != "0" ]; then
    log_error "diagnostics total changed after a read-only validate call: total=${final_total}"
    diag_snapshot
    exit 1
fi
log_success "validate call persisted nothing (conf dir and diagnostics registry unchanged)"

log_section "PASS"
TEST_PASSED=1
exit 0
