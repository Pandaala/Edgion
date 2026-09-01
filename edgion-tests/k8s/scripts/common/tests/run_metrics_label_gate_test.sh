#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../run_metrics_label_gate.sh"
TEST_ROOT="$(mktemp -d /tmp/edgion-metrics-label-gate-test.XXXXXX)"
FAKE_BIN="${TEST_ROOT}/bin"
ENDPOINT_FIXTURE="${TEST_ROOT}/endpoints.json"
EXEC_LOG="${TEST_ROOT}/exec.log"

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
    case "${2:-}" in
      --raw|endpointslices.discovery.k8s.io)
        cat "$FAKE_ENDPOINT_FIXTURE"
        ;;
      pods)
        if [ "${FAKE_NO_CLIENT_POD:-false}" != "true" ]; then
          printf '%s\n' 'client-old <none>' 'client-new <none>'
        fi
        ;;
      *)
        echo "unexpected kubectl get resource: ${2:-}" >&2
        exit 9
        ;;
    esac
    ;;
  exec)
    printf '%s\n' "$*" >> "$FAKE_EXEC_LOG"
    url="${!#}"
    case "$url" in
      http://10.0.0.2:12090/metrics)
        printf '%s\n' '# HELP first_metric example' 'first_metric{role="controller"} 1'
        ;;
      http://10.0.0.3:12090/metrics)
        case "${FAKE_SECOND_RESPONSE:-valid}" in
          valid) printf '%s\n' '# HELP second_metric example' 'second_metric 1' ;;
          bad-role) printf '%s\n' 'second_metric{role="gateway"} 1' ;;
          empty) : ;;
          error) exit 22 ;;
        esac
        ;;
      *)
        echo "unexpected metrics URL: $url" >&2
        exit 9
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
export FAKE_ENDPOINT_FIXTURE="$ENDPOINT_FIXTURE"
export FAKE_EXEC_LOG="$EXEC_LOG"
export INVENTORY_TIMEOUT_SECS=0
export PROBE_TIMEOUT_SECS=3

write_endpoints() {
  local include_endpoints="$1"
  if [ "$include_endpoints" = "true" ]; then
    cat > "$ENDPOINT_FIXTURE" <<'EOF'
{
  "apiVersion": "discovery.k8s.io/v1",
  "kind": "EndpointSliceList",
  "items": [{
    "metadata": {"name": "gateway-metrics"},
    "addressType": "IPv4",
    "ports": [{"name": "metrics-12090", "protocol": "TCP", "port": 12090}],
    "endpoints": [
      {"addresses": ["10.0.0.3"], "conditions": {"ready": true}},
      {"addresses": ["10.0.0.2"], "conditions": {"ready": true, "terminating": false}},
      {"addresses": ["10.0.0.9"], "conditions": {"ready": false}},
      {"addresses": ["10.0.0.8"], "conditions": {"ready": true, "terminating": true}}
    ]
  }]
}
EOF
  else
    cat > "$ENDPOINT_FIXTURE" <<'EOF'
{"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","items":[]}
EOF
  fi
}

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
  local expected_status="$1"
  local expected_endpoints="$2"
  local expected_checked="$3"
  local output_file="$4"
  local expected_failed=1
  if [ "$expected_status" = "PASS" ]; then
    expected_failed=0
  fi
  expected="@@FINAL_GATE_RESULT MetricsLabelWhitelist ${expected_status} endpoints=${expected_endpoints} checked=${expected_checked}
@@FINAL_GATE_SUMMARY total=1 failed=${expected_failed}"
  actual="$(cat "$output_file")"
  if [ "$actual" != "$expected" ]; then
    echo "Unexpected result protocol" >&2
    printf 'Expected:\n%s\nActual:\n%s\n' "$expected" "$actual" >&2
    exit 1
  fi
}

OUTPUT_FILE="${TEST_ROOT}/stdout"
ERROR_FILE="${TEST_ROOT}/stderr"

write_endpoints true
: > "$EXEC_LOG"
unset FAKE_SECOND_RESPONSE FAKE_NO_CLIENT_POD
run_case 0 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol PASS 2 2 "$OUTPUT_FILE"
[ "$(wc -l < "$EXEC_LOG" | tr -d ' ')" -eq 2 ]
grep -q -- '--namespace edgion-test --container test-client client-new -- curl' "$EXEC_LOG"
grep -q 'http://10.0.0.2:12090/metrics' "$EXEC_LOG"
grep -q 'http://10.0.0.3:12090/metrics' "$EXEC_LOG"

export FAKE_SECOND_RESPONSE=bad-role
: > "$EXEC_LOG"
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 2 "$OUTPUT_FILE"
grep -q 'unexpected role label' "$ERROR_FILE"
[ "$(wc -l < "$EXEC_LOG" | tr -d ' ')" -eq 2 ]

export FAKE_SECOND_RESPONSE=empty
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 2 "$OUTPUT_FILE"
grep -q 'returned an empty body' "$ERROR_FILE"

export FAKE_SECOND_RESPONSE=error
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 2 "$OUTPUT_FILE"
grep -q 'could not be read' "$ERROR_FILE"

unset FAKE_SECOND_RESPONSE
export FAKE_NO_CLIENT_POD=true
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 0 "$OUTPUT_FILE"
grep -q 'No running, non-terminating test-client Pod found' "$ERROR_FILE"

unset FAKE_NO_CLIENT_POD
write_endpoints false
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 0 0 "$OUTPUT_FILE"
grep -q 'EndpointSlice inventory failed' "$ERROR_FILE"

echo "run_metrics_label_gate_test.sh: PASS"
