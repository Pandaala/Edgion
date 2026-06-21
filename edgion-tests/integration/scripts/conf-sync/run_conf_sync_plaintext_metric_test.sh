#!/bin/bash
# =============================================================================
# Asserts the conf_sync plaintext_default system-error metric:
#   - PLAINTEXT run (EDGION_CONF_SYNC_PLAINTEXT=1): present on BOTH ends.
#   - mTLS run (default):                            absent on BOTH ends.
# =============================================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UTILS_DIR="${SCRIPT_DIR}/../utils"

GW_METRICS="http://127.0.0.1:12090/metrics"
CTRL_METRICS="http://127.0.0.1:12190/metrics"
PATTERN='edgion_system_error_state\{[^}]*reason="plaintext_default"'

PASS=0; FAIL=0
ok()   { echo "[PASS] $1"; PASS=$((PASS+1)); }
bad()  { echo "[FAIL] $1"; FAIL=$((FAIL+1)); }

# Wait up to N s for $1 (url) to contain the pattern.
wait_present() {
    local url="$1" label="$2" t=0
    while [ $t -lt 30 ]; do
        if curl -sf "$url" 2>/dev/null | grep -E -q -- "$PATTERN"; then
            ok "plaintext_default present on $label"; return 0
        fi
        sleep 1; t=$((t+1))
    done
    bad "plaintext_default NOT present on $label (expected)"; return 1
}

# Assert $1 (url) does NOT contain the pattern (single scrape after settle).
assert_absent() {
    local url="$1" label="$2"
    # Guard: the endpoint must be reachable, else "absent" is meaningless.
    if ! curl -sf "$url" >/dev/null 2>&1; then
        bad "metrics endpoint unreachable on $label (cannot assert absent)"
        return 1
    fi
    if curl -sf "$url" 2>/dev/null | grep -E -q -- "$PATTERN"; then
        bad "plaintext_default present on $label (expected ABSENT under mTLS)"
    else
        ok "plaintext_default absent on $label"
    fi
}

# ---- Phase 1: plaintext run → metric PRESENT on both ends ----
EDGION_CONF_SYNC_PLAINTEXT=1 "${UTILS_DIR}/start_all_with_conf.sh" --suites HTTPRoute/Basic
wait_present "$GW_METRICS"   "gateway(client)"
wait_present "$CTRL_METRICS" "controller(server)"
"${UTILS_DIR}/kill_all.sh" >/dev/null 2>&1 || true
sleep 5

# ---- Phase 2: mTLS run (default) → metric ABSENT on both ends ----
"${UTILS_DIR}/start_all_with_conf.sh" --suites HTTPRoute/Basic
sleep 5  # let both channels establish
assert_absent "$GW_METRICS"   "gateway(client)"
assert_absent "$CTRL_METRICS" "controller(server)"
"${UTILS_DIR}/kill_all.sh" >/dev/null 2>&1 || true

echo "=== plaintext_default metric test: ${PASS} passed, ${FAIL} failed ==="
[ "$FAIL" -eq 0 ]
