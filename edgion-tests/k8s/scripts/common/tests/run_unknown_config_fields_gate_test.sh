#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../run_unknown_config_fields_gate.sh"
TEST_ROOT="$(mktemp -d /tmp/edgion-unknown-fields-gate-test.XXXXXX)"
FAKE_BIN="${TEST_ROOT}/bin"
CALL_LOG="${TEST_ROOT}/kubectl.log"

cleanup() {
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT INT TERM

mkdir -p "$FAKE_BIN"
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
  get)
    if [ "${FAKE_NO_CLIENT_POD:-false}" != "true" ]; then
      printf '%s\n' 'client-old <none>' 'client-new <none>'
    fi
    ;;
  exec)
    printf '%s\n' "$*" >> "$FAKE_KUBECTL_LOG"
    case " $* " in
      *" --header Authorization: Bearer integration-controller-admin-token-0123456789 "*) ;;
      *) echo "missing canonical bearer token" >&2; exit 9 ;;
    esac
    url="${!#}"
    [ "$url" = 'http://edgion-controller.edgion-system.svc.cluster.local:12101/api/v1/diagnostics/unknown-fields' ] \
      || { echo "unexpected URL: $url" >&2; exit 9; }
    case "${FAKE_RESPONSE_MODE:-zero}" in
      zero)
        printf '%s\n' '{"success":true,"data":{"total":0,"truncated":false,"items":[]}}' '200'
        ;;
      findings)
        printf '%s\n' '{"success":true,"data":{"total":2,"truncated":false,"items":[{"kind":"EdgionPlugins","namespace":"edgion-test","name":"dynamic-a","paths":["requestPlugins[0].config.typo"]},{"kind":"LinkSys","namespace":"edgion-test","name":"dynamic-b","paths":["config.timeout.conect"]}]}}' '200'
        ;;
      truncated)
        printf '%s\n' '{"success":true,"data":{"total":101,"truncated":true,"items":[{"kind":"EdgionPlugins","namespace":"edgion-test","name":"first","paths":["requestPlugins[0].config.typo"]}]}}' '200'
        ;;
      inconsistent)
        printf '%s\n' '{"success":true,"data":{"total":2,"truncated":false,"items":[]}}' '200'
        ;;
      malformed)
        printf '%s\n' '{not-json' '200'
        ;;
      empty)
        printf '\n200\n'
        ;;
      unauthorized)
        printf '%s\n' '{"success":false}' '401'
        ;;
      unreachable)
        exit 7
        ;;
    esac
    ;;
  *)
    echo "unexpected kubectl command: $*" >&2
    exit 9
    ;;
esac
EOF
chmod 755 "${FAKE_BIN}/kubectl"

export PATH="${FAKE_BIN}:${PATH}"
export FAKE_KUBECTL_LOG="$CALL_LOG"

run_case() {
  local expected_rc="$1"
  local output_file="$2"
  local error_file="$3"
  set +e
  "$HELPER" > "$output_file" 2> "$error_file"
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
  local total="$2"
  local items="$3"
  local truncated="$4"
  local output_file="$5"
  local failed=1
  if [ "$status" = "PASS" ]; then
    failed=0
  fi
  expected="@@FINAL_GATE_RESULT UnknownConfigFields ${status} total=${total} items=${items} truncated=${truncated}
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

: > "$CALL_LOG"
unset FAKE_RESPONSE_MODE FAKE_NO_CLIENT_POD
run_case 0 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol PASS 0 0 false "$OUTPUT_FILE"
grep -q -- '--namespace edgion-test --container test-client client-new -- curl' "$CALL_LOG"

export FAKE_RESPONSE_MODE=findings
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 2 false "$OUTPUT_FILE"
grep -q 'EdgionPlugins/edgion-test/dynamic-a -> requestPlugins\[0\].config.typo' "$ERROR_FILE"
grep -q 'LinkSys/edgion-test/dynamic-b -> config.timeout.conect' "$ERROR_FILE"

export FAKE_RESPONSE_MODE=truncated
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 101 1 true "$OUTPUT_FILE"
grep -q 'truncated=true' "$ERROR_FILE"

for mode in inconsistent malformed empty unauthorized unreachable; do
  export FAKE_RESPONSE_MODE="$mode"
  run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
  assert_protocol FAIL unknown unknown unknown "$OUTPUT_FILE"
done

unset FAKE_RESPONSE_MODE
export FAKE_NO_CLIENT_POD=true
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL unknown unknown unknown "$OUTPUT_FILE"
grep -q 'No running, non-terminating test-client Pod found' "$ERROR_FILE"

echo "run_unknown_config_fields_gate_test.sh: PASS"
