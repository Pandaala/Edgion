#!/bin/bash
# =============================================================================
# Asserts the conf_sync plaintext posture (fail-close since 2026-07-14):
#   1. PLAINTEXT run (EDGION_CONF_SYNC_PLAINTEXT=1 -> explicit `skip_tls: true`):
#      edgion_system_error_state{reason="skip_tls"} present on BOTH ends.
#   2. mTLS run (default): that metric is absent on BOTH ends.
#   3. FAIL-CLOSE: a config with `conf_sync_security` omitted entirely makes both
#      binaries refuse to start (non-zero exit). Plaintext-by-omission no longer
#      runs at all, which is why the old `plaintext_default` reason was retired.
# =============================================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UTILS_DIR="${SCRIPT_DIR}/../utils"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# Single implementation of the `conf_sync_security:` strip lives here (used below by
# assert_fail_close to reproduce the omitted-section state from the real configs).
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

GW_METRICS="http://127.0.0.1:12090/metrics"
CTRL_METRICS="http://127.0.0.1:12190/metrics"
PATTERN='edgion_system_error_state\{[^}]*reason="skip_tls"'

PASS=0; FAIL=0
ok()   { echo "[PASS] $1"; PASS=$((PASS+1)); }
bad()  { echo "[FAIL] $1"; FAIL=$((FAIL+1)); }

# Wait up to N s for $1 (url) to contain the pattern.
wait_present() {
    local url="$1" label="$2" t=0
    while [ $t -lt 30 ]; do
        if curl -sf "$url" 2>/dev/null | grep -E -q -- "$PATTERN"; then
            ok "skip_tls present on $label"; return 0
        fi
        sleep 1; t=$((t+1))
    done
    bad "skip_tls NOT present on $label (expected)"; return 1
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
        bad "skip_tls present on $label (expected ABSENT under mTLS)"
    else
        ok "skip_tls absent on $label"
    fi
}

# Run "$@", killing it after $1 seconds. GNU `timeout` is NOT present on macOS (and
# `gtimeout` only with coreutils), where an unguarded `timeout ...` would exit 127 and
# make the fail-close assertion below pass/fail for the wrong reason.
run_with_timeout() {
    local secs="$1"; shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$secs" "$@"
        return $?
    fi
    if command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$secs" "$@"
        return $?
    fi
    # Portable fallback: watchdog SIGKILLs the child if it outlives the budget.
    "$@" &
    local pid=$! rc
    ( sleep "$secs"; kill -9 "$pid" 2>/dev/null ) &
    local watchdog=$!
    wait "$pid"; rc=$?
    kill "$watchdog" 2>/dev/null
    wait "$watchdog" 2>/dev/null
    return $rc
}

# Run a binary against a config whose conf_sync_security block was stripped: it must
# exit non-zero with the actionable message instead of running the Secret-carrying
# channel in plaintext.
assert_fail_close() {
    local bin="$1" base_config="$2" label="$3"
    local bin_path="${PROJECT_ROOT}/target/debug/${bin}"
    # A missing binary is a FAIL, never a silent skip: this is the headline regression test
    # for the conf_sync fail-close, and a skip that increments neither counter would let the
    # script print "N passed, 0 failed" and exit 0 without ever having tested the fail-close.
    # Phases 1-2 already require both binaries, so an absent one means a broken run, not an
    # unsupported environment.
    if [ ! -x "$bin_path" ]; then
        bad "${label}: ${bin_path} not built — cannot verify fail-close (run cargo build first)"
        return 1
    fi
    local tmp_config
    tmp_config="$(mktemp)"
    # Drop the top-level conf_sync_security mapping -> reproduce the omitted state.
    cp "$base_config" "$tmp_config"
    conf_sync_strip_security_block "$tmp_config"

    # `--config-file` (alias `-c`) is the real flag on BOTH binaries; clap v4 does not
    # infer, so a `--config` here would abort with rc=2 "unexpected argument" and the
    # assertion below would silently be testing argument parsing, not the fail-close.
    local out rc
    out="$(cd "$PROJECT_ROOT" && run_with_timeout 30 "$bin_path" --config-file "$tmp_config" 2>&1)"
    rc=$?
    rm -f "$tmp_config"
    if [ "$rc" -eq 0 ]; then
        bad "${label}: started with conf_sync_security omitted (expected non-zero exit)"
        return 1
    fi
    if echo "$out" | grep -q "conf_sync_security is not configured"; then
        ok "${label}: refused to start (exit ${rc}) with the actionable fail-close error"
    else
        bad "${label}: exited ${rc} but without the fail-close message; output: ${out}"
    fi
}

# ---- Phase 1: explicit skip_tls run -> metric PRESENT on both ends ----
EDGION_CONF_SYNC_PLAINTEXT=1 "${UTILS_DIR}/start_all_with_conf.sh" --suites HTTPRoute/Basic
wait_present "$GW_METRICS"   "gateway(client)"
wait_present "$CTRL_METRICS" "controller(server)"
"${UTILS_DIR}/kill_all.sh" >/dev/null 2>&1 || true
sleep 5

# ---- Phase 2: mTLS run (default) -> metric ABSENT on both ends ----
"${UTILS_DIR}/start_all_with_conf.sh" --suites HTTPRoute/Basic
sleep 5  # let both channels establish
assert_absent "$GW_METRICS"   "gateway(client)"
assert_absent "$CTRL_METRICS" "controller(server)"
"${UTILS_DIR}/kill_all.sh" >/dev/null 2>&1 || true

# ---- Phase 3: omitted conf_sync_security -> both binaries refuse to start ----
assert_fail_close edgion-gateway    "${PROJECT_ROOT}/config/edgion-gateway.yaml"    "gateway"
assert_fail_close edgion-controller "${PROJECT_ROOT}/config/edgion-controller.yaml" "controller"

echo "=== conf_sync skip_tls / fail-close test: ${PASS} passed, ${FAIL} failed ==="
[ "$FAIL" -eq 0 ]
