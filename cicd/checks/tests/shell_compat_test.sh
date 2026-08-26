#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../../edgion-tests/integration/scripts/utils/shell_compat.sh
source "${SCRIPT_DIR}/../../../edgion-tests/integration/scripts/utils/shell_compat.sh"

case "$EDGION_SHELL_PLATFORM" in
    darwin|linux) ;;
    *) echo "ERROR: unexpected platform adapter: $EDGION_SHELL_PLATFORM" >&2; exit 1 ;;
esac

_edgion_test_dispatch_darwin() { printf 'darwin\n'; }
_edgion_test_dispatch_linux() { printf 'linux\n'; }
[ "$(edgion_platform_call test_dispatch)" = "$EDGION_SHELL_PLATFORM" ]

[ "$(printf '' | edgion_count_nonempty_lines)" = "0" ]
[ "$(printf 'one\n\ntwo\n' | edgion_count_nonempty_lines)" = "2" ]

large_integer="9223372036854775807"
parsed_integer=$(printf '{"data":{"id":%s}}' "$large_integer" | edgion_json_integer 0 data id)
[ "$parsed_integer" = "$large_integer" ]
[ "$(printf '{}' | edgion_json_integer 7 data id)" = "7" ]

attempts=0
succeed_on_third_attempt() {
    attempts=$((attempts + 1))
    [ "$attempts" -ge 3 ]
}
edgion_wait_until 4 1 succeed_on_third_attempt
if edgion_wait_until 0 1 false; then
    echo "ERROR: wait helper accepted a command that never succeeded" >&2
    exit 1
fi

sleep 10 &
sleep_pid=$!
report_test_dir=""
test_cleanup() {
    kill "$sleep_pid" 2>/dev/null || true
    wait "$sleep_pid" 2>/dev/null || true
    if [ -n "$report_test_dir" ] && [ -d "$report_test_dir" ]; then
        rm -rf "$report_test_dir"
    fi
}
trap test_cleanup EXIT INT TERM
if ! edgion_live_pids_by_name sleep | grep -qx "$sleep_pid"; then
    echo "ERROR: live-process helper did not return the test process" >&2
    exit 1
fi

[ "$BASH_ENV" = "$EDGION_SHELL_COMPAT_FILE" ]
if ! bash -c 'command -v edgion_platform_call >/dev/null && command -v edgion_wait_until >/dev/null && command -v edgion_json_integer >/dev/null'; then
    echo "ERROR: BASH_ENV did not bootstrap helpers in a child Bash process" >&2
    exit 1
fi

RUNNER="${SCRIPT_DIR}/../../../edgion-tests/integration/scripts/integration/run_integration.sh"
# shellcheck source=../../../edgion-tests/integration/scripts/integration/run_integration.sh
source "$RUNNER"

saved_ctrl_admin_token="${CTRL_ADMIN_TOKEN:-}"
saved_edgion_admin_token="${EDGION_ADMIN_TOKEN:-}"
CTRL_ADMIN_TOKEN=""
EDGION_ADMIN_TOKEN=""
curl() { return 22; }
if verify_metrics_labels "http://127.0.0.1:1/metrics" "test" >/dev/null; then
    echo "ERROR: unreachable metrics endpoint passed mandatory validation" >&2
    exit 1
fi
if verify_no_unknown_config_fields "http://127.0.0.1:1" >/dev/null; then
    echo "ERROR: missing admin authentication passed mandatory validation" >&2
    exit 1
fi
CTRL_ADMIN_TOKEN="test-token"
curl() { printf '%s\n%s\n' '{"success":true,"data":{"total":0,"truncated":false,"items":[]}}' '200'; }
if ! verify_no_unknown_config_fields "http://127.0.0.1:1" >/dev/null; then
    echo "ERROR: valid empty unknown-fields response failed validation" >&2
    exit 1
fi
curl() { printf '%s\n%s\n' '{"success":true,"data":{"total":0,"truncated":false,"items":[null]}}' '200'; }
if verify_no_unknown_config_fields "http://127.0.0.1:1" >/dev/null; then
    echo "ERROR: malformed unknown-fields item passed mandatory validation" >&2
    exit 1
fi
curl() { printf '%s\n%s\n' '{"success":false,"data":{"total":0,"truncated":false,"items":[]}}' '200'; }
if verify_no_unknown_config_fields "http://127.0.0.1:1" >/dev/null; then
    echo "ERROR: unsuccessful unknown-fields envelope passed mandatory validation" >&2
    exit 1
fi
unset -f curl
CTRL_ADMIN_TOKEN="$saved_ctrl_admin_token"
EDGION_ADMIN_TOKEN="$saved_edgion_admin_token"

report_test_dir=$(mktemp -d)

deadline_child_file="${report_test_dir}/deadline-child.pid"
set +e
python3 "$DEADLINE_RUNNER" --timeout-secs 1 --grace-secs 1 -- \
    bash -c 'trap "" TERM; sleep 30 & child=$!; echo "$child" > "$1"; wait "$child"' bash "$deadline_child_file" \
    >/dev/null 2>&1
deadline_rc=$?
set -e
if [ "$deadline_rc" -ne 124 ]; then
    echo "ERROR: deadline runner returned ${deadline_rc}, expected 124" >&2
    exit 1
fi
deadline_child_pid=$(cat "$deadline_child_file")
deadline_child_gone() { ! kill -0 "$deadline_child_pid" 2>/dev/null; }
if ! edgion_wait_until 3 1 deadline_child_gone; then
    echo "ERROR: deadline runner left child process ${deadline_child_pid} alive" >&2
    exit 1
fi
unset -f deadline_child_gone

set +e
python3 "$DEADLINE_RUNNER" --timeout-secs 3 --grace-secs 1 -- bash -c 'exit 124' >/dev/null 2>&1
reserved_rc=$?
set -e
if [ "$reserved_rc" -ne 125 ]; then
    echo "ERROR: child exit 124 was not distinguished from a watchdog timeout" >&2
    exit 1
fi

forwarded_child_file="${report_test_dir}/forwarded-child.pid"
python3 "$DEADLINE_RUNNER" --timeout-secs 30 --grace-secs 1 -- \
    bash -c 'trap "" TERM; sleep 30 & child=$!; echo "$child" > "$1"; wait "$child"' bash "$forwarded_child_file" \
    >/dev/null 2>&1 &
forwarded_watchdog_pid=$!
forwarded_child_ready() { [ -s "$forwarded_child_file" ]; }
if ! edgion_wait_until 3 1 forwarded_child_ready; then
    echo "ERROR: signal-forwarding test child did not start" >&2
    exit 1
fi
unset -f forwarded_child_ready
kill -TERM "$forwarded_watchdog_pid"
set +e
wait "$forwarded_watchdog_pid"
forwarded_rc=$?
set -e
if [ "$forwarded_rc" -ne 143 ]; then
    echo "ERROR: externally terminated watchdog returned ${forwarded_rc}, expected 143" >&2
    exit 1
fi
forwarded_child_pid=$(cat "$forwarded_child_file")
if kill -0 "$forwarded_child_pid" 2>/dev/null; then
    echo "ERROR: externally terminated watchdog left child ${forwarded_child_pid} alive" >&2
    exit 1
fi

WORK_DIR="$report_test_dir"
REPORT_FILE="${report_test_dir}/pending-report.log"
REPORT_ENTRIES=()
REPORT_FINALIZED=false
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0
SKIPPED_TESTS=0
TIMED_OUT_TESTS=0
write_report "INCOMPLETE" "$REPORT_FILE" "not-created"
report_test "Isolated_example" "PASS" "2"
init_report
[ ! -e "${report_test_dir}/pending-report.log" ]
grep -q '^Status: INCOMPLETE$' "$REPORT_FILE"
grep -q '^\[PASS\] Isolated_example (2s)$' "$REPORT_FILE"
report_test "Final_example" "FAIL" "0" "expected regression"
report_test "Timeout_example" "TIMEOUT" "3" "deadline exceeded"
finalize_report "FAIL" >/dev/null
[ "$REPORT_FINALIZED" = "true" ]
grep -q '^Status: FAIL$' "$REPORT_FILE"
grep -q '^Failed: 2$' "$REPORT_FILE"
grep -q '^Timed out: 1$' "$REPORT_FILE"
grep -q '^\[FAIL\] Final_example (0s) - expected regression$' "$REPORT_FILE"
grep -q '^\[TIMEOUT\] Timeout_example (3s) - deadline exceeded$' "$REPORT_FILE"
if find "$report_test_dir" -maxdepth 1 -name 'report.log.tmp.*' -print -quit | grep -q .; then
    echo "ERROR: atomic report writer left a temporary file" >&2
    exit 1
fi

abort_report="${report_test_dir}/aborted-report.log"
set +e
(
    DO_CLEANUP=false
    REPORT_FILE="$abort_report"
    REPORT_ENTRIES=()
    REPORT_FINALIZED=false
    REPORT_PHASE="shell compatibility test"
    TOTAL_TESTS=0
    PASSED_TESTS=0
    FAILED_TESTS=0
    SKIPPED_TESTS=0
    TIMED_OUT_TESTS=0
    write_report "INCOMPLETE" "$REPORT_FILE" "not-created"
    trap cleanup EXIT
    exit 7
) >/dev/null
abort_rc=$?
set -e
[ "$abort_rc" -eq 7 ]
grep -q '^Status: INCOMPLETE$' "$abort_report"
grep -q '^\[FAIL\] Run_Aborted (0s) - phase=shell compatibility test; exit=7$' "$abort_report"
echo "OK: shared shell compatibility helpers passed"
