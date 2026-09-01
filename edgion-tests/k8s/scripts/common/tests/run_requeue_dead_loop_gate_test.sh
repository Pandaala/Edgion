#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../run_requeue_dead_loop_gate.sh"
TEST_ROOT="$(mktemp -d /tmp/edgion-requeue-gate-test.XXXXXX)"
FAKE_BIN="${TEST_ROOT}/bin"
POD_FIXTURE="${TEST_ROOT}/pods.json"
CALL_LOG="${TEST_ROOT}/kubectl.log"
RUN_START='2026-08-30T01:02:03Z'

cleanup() {
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT INT TERM

mkdir -p "$FAKE_BIN"
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >> "$FAKE_KUBECTL_LOG"
case "${1:-}" in
  get)
    [ "${2:-}" = "pods" ] || exit 9
    cat "$FAKE_POD_FIXTURE"
    ;;
  logs)
    args=" $* "
    case "$args" in
      *" --since-time ${EXPECTED_RUN_START} "*) ;;
      *) echo "missing or wrong --since-time: $*" >&2; exit 9 ;;
    esac
    pod="${!#}"
    previous=false
    case "$args" in
      *" --previous "*) previous=true ;;
    esac
    if [ "$pod" = "controller-a" ] && [ "$previous" = "true" ]; then
      echo "unexpected previous-log request for non-restarted Pod" >&2
      exit 9
    fi
    if [ "${FAKE_LOG_MODE:-pass}" = "current-error" ] \
      && [ "$pod" = "controller-b" ] && [ "$previous" = "false" ]; then
      exit 7
    fi
    if [ "${FAKE_LOG_MODE:-pass}" = "previous-error" ] \
      && [ "$pod" = "controller-b" ] && [ "$previous" = "true" ]; then
      exit 8
    fi
    if [ "${FAKE_LOG_MODE:-pass}" = "cycle-current" ] \
      && [ "$pod" = "controller-a" ] && [ "$previous" = "false" ]; then
      echo 'Trigger cycle limit reached, dropping requeue'
    elif [ "${FAKE_LOG_MODE:-pass}" = "depth-previous" ] \
      && [ "$pod" = "controller-b" ] && [ "$previous" = "true" ]; then
      echo 'Trigger chain depth limit reached, dropping requeue'
    else
      echo 'ordinary bounded controller log line'
    fi
    ;;
  *)
    echo "unexpected kubectl command: $*" >&2
    exit 9
    ;;
esac
EOF
chmod 755 "${FAKE_BIN}/kubectl"

export PATH="${FAKE_BIN}:${PATH}"
export FAKE_POD_FIXTURE="$POD_FIXTURE"
export FAKE_KUBECTL_LOG="$CALL_LOG"
export EXPECTED_RUN_START="$RUN_START"

write_pods() {
  cat > "$POD_FIXTURE" <<'EOF'
{
  "apiVersion": "v1",
  "kind": "List",
  "items": [
    {
      "apiVersion": "v1",
      "kind": "Pod",
      "metadata": {"name": "controller-a"},
      "status": {"containerStatuses": [{"name": "edgion-controller", "restartCount": 0}]}
    },
    {
      "apiVersion": "v1",
      "kind": "Pod",
      "metadata": {"name": "controller-b"},
      "status": {"containerStatuses": [{"name": "edgion-controller", "restartCount": 2}]}
    }
  ]
}
EOF
}

run_case() {
  local expected_rc="$1"
  local output_file="$2"
  local error_file="$3"
  set +e
  "$HELPER" --run-start-time "$RUN_START" > "$output_file" 2> "$error_file"
  local actual_rc=$?
  set -e
  if [ "$actual_rc" -ne "$expected_rc" ]; then
    echo "Expected rc=$expected_rc, got rc=$actual_rc" >&2
    cat "$output_file" >&2
    cat "$error_file" >&2
    exit 1
  fi
}

assert_protocol() {
  local status="$1"
  local pods="$2"
  local streams="$3"
  local matches="$4"
  local output_file="$5"
  local failed=1
  if [ "$status" = "PASS" ]; then
    failed=0
  fi
  expected="@@FINAL_GATE_RESULT RequeueDeadLoop ${status} pods=${pods} streams=${streams} matches=${matches}
@@FINAL_GATE_SUMMARY total=1 failed=${failed}"
  actual="$(cat "$output_file")"
  if [ "$actual" != "$expected" ]; then
    echo "Unexpected result protocol" >&2
    printf 'Expected:\n%s\nActual:\n%s\n' "$expected" "$actual" >&2
    exit 1
  fi
}

OUTPUT_FILE="${TEST_ROOT}/stdout"
ERROR_FILE="${TEST_ROOT}/stderr"

write_pods
: > "$CALL_LOG"
unset FAKE_LOG_MODE
run_case 0 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol PASS 2 3 0 "$OUTPUT_FILE"
[ "$(grep -c '^logs ' "$CALL_LOG")" -eq 3 ]
[ "$(grep -c -- '--previous' "$CALL_LOG")" -eq 1 ]
[ "$(grep -c -- "--since-time ${RUN_START}" "$CALL_LOG")" -eq 3 ]

export FAKE_LOG_MODE=depth-previous
: > "$CALL_LOG"
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 3 1 "$OUTPUT_FILE"
grep -q 'Trigger chain depth limit reached, dropping requeue' "$ERROR_FILE"

export FAKE_LOG_MODE=cycle-current
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 3 1 "$OUTPUT_FILE"
grep -q 'Trigger cycle limit reached, dropping requeue' "$ERROR_FILE"

export FAKE_LOG_MODE=current-error
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 2 0 "$OUTPUT_FILE"
grep -q 'Failed to retrieve current Controller logs for Pod controller-b' "$ERROR_FILE"

export FAKE_LOG_MODE=previous-error
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 2 0 "$OUTPUT_FILE"
grep -q 'Failed to retrieve previous Controller logs for Pod controller-b' "$ERROR_FILE"

cat > "$POD_FIXTURE" <<'EOF'
{"apiVersion":"v1","kind":"List","items":[]}
EOF
unset FAKE_LOG_MODE
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 0 0 0 "$OUTPUT_FILE"
grep -q 'No Controller Pods found' "$ERROR_FILE"

cat > "$POD_FIXTURE" <<'EOF'
{
  "apiVersion":"v1",
  "kind":"List",
  "items":[{"apiVersion":"v1","kind":"Pod","metadata":{"name":"controller-a"},"status":{"containerStatuses":[]}}]
}
EOF
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 1 0 0 "$OUTPUT_FILE"
grep -q 'missing or invalid container status' "$ERROR_FILE"

set +e
"$HELPER" --run-start-time '2026-08-30 01:02:03' > "$OUTPUT_FILE" 2> "$ERROR_FILE"
invalid_time_rc=$?
set -e
[ "$invalid_time_rc" -eq 2 ]
[ ! -s "$OUTPUT_FILE" ]
grep -q 'must be an RFC3339 timestamp' "$ERROR_FILE"

echo "run_requeue_dead_loop_gate_test.sh: PASS"
