#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../run_cache_residue_gate.sh"
TEST_ROOT="$(mktemp -d /tmp/edgion-cache-residue-gate-test.XXXXXX)"
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

printf '%s\n' "$*" >> "$FAKE_KUBECTL_LOG"
case "${1:-}" in
  get)
    case "${2:-}" in
      pods)
        if [ "${FAKE_NO_CLIENT_POD:-false}" != "true" ]; then
          echo 'client-a <none>'
        fi
        ;;
      configmaps)
        if [ "${FAKE_ROOT_CA_QUERY_FAIL:-false}" = "true" ]; then
          exit 8
        fi
        cat <<'JSON'
{"apiVersion":"v1","kind":"ConfigMapList","items":[{"metadata":{"namespace":"app","name":"kube-root-ca.crt","annotations":{"kubernetes.io/description":"Contains a CA bundle that can be used to verify the kube-apiserver when using internal endpoints such as the internal service IP or kubernetes.default.svc. No other usage is guaranteed across distributions of Kubernetes clusters."}},"data":{"ca.crt":"test-ca"}},{"metadata":{"namespace":"app2","name":"kube-root-ca.crt","annotations":{"kubernetes.io/description":"Contains a CA bundle that can be used to verify the kube-apiserver when using internal endpoints such as the internal service IP or kubernetes.default.svc. No other usage is guaranteed across distributions of Kubernetes clusters."}},"data":{"ca.crt":"test-ca"}},{"metadata":{"namespace":"spoofed","name":"kube-root-ca.crt","annotations":{"kubernetes.io/description":"not Kubernetes-owned"}},"data":{"ca.crt":"test-ca"}},{"metadata":{"namespace":"extra-data","name":"kube-root-ca.crt","annotations":{"kubernetes.io/description":"Contains a CA bundle that can be used to verify the kube-apiserver when using internal endpoints such as the internal service IP or kubernetes.default.svc. No other usage is guaranteed across distributions of Kubernetes clusters."}},"data":{"ca.crt":"test-ca","leak":"must remain visible"}}]}
JSON
        ;;
      secrets)
        if [ "${FAKE_CONF_SYNC_SECRET_QUERY_FAIL:-false}" = "true" ]; then
          exit 8
        fi
        cat <<'JSON'
{"apiVersion":"v1","kind":"SecretList","items":[{"metadata":{"namespace":"edgion-system","name":"edgion-conf-sync-ca","labels":{"edgion.io/managed-by":"conf-sync-ca"}},"type":"Opaque","data":{"ca.crt":"Y2E=","ca.key":"a2V5"}},{"metadata":{"namespace":"edgion-system","name":"edgion-conf-sync-certs","labels":{"edgion.io/managed-by":"conf-sync-ca"}},"type":"Opaque","data":{"ca.crt":"Y2E=","client.crt":"Y3J0","client.key":"a2V5"}},{"metadata":{"namespace":"edgion-system","name":"edgion-conf-sync-ca","labels":{"edgion.io/managed-by":"spoofed"}},"type":"Opaque","data":{"ca.crt":"Y2E=","ca.key":"a2V5"}},{"metadata":{"namespace":"edgion-system","name":"edgion-conf-sync-certs","labels":{"edgion.io/managed-by":"conf-sync-ca"}},"type":"Opaque","data":{"ca.crt":"Y2E=","client.crt":"Y3J0","client.key":"a2V5","extra":"bGVhaw=="}}]}
JSON
        ;;
      --raw)
        args=" $* "
        if echo "$args" | grep -q 'service-name%3Dedgion-controller'; then
          service=controller
          port_name=http
          port=12101
          address=10.0.0.10
        elif echo "$args" | grep -q 'service-name%3Dedgion-gateway'; then
          service=gateway
          port_name=admin-12001
          port=12001
          address=10.0.0.20
        else
          echo "unexpected EndpointSlice selector: $*" >&2
          exit 9
        fi
        if [ "${FAKE_MISSING_ENDPOINTS:-}" = "$service" ]; then
          echo '{"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","items":[]}'
        else
          printf '{"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","items":[{"metadata":{"name":"%s"},"addressType":"IPv4","ports":[{"name":"%s","protocol":"TCP","port":%s}],"endpoints":[{"addresses":["%s"],"conditions":{"ready":true}}]}]}\n' \
            "$service" "$port_name" "$port" "$address"
        fi
        ;;
      *) exit 9 ;;
    esac
    ;;
  exec)
    args=" $* "
    case "$args" in
      *" -- test -x /usr/local/bin/cache_residue ")
        [ "${FAKE_MISSING_BINARY:-false}" != "true" ]
        ;;
      *" -- /usr/local/bin/cache_residue "*)
        for expected in \
          '--controller-addr http://10.0.0.10:12101' \
          '--gateway-addr http://10.0.0.20:12001' \
          '--controller-token integration-controller-admin-token-0123456789' \
          '--gateway-token integration-gateway-admin-token-0123456789' \
          '--timeout-secs 75' \
          "--wipe-timeout-secs ${FAKE_EXPECTED_WIPE_TIMEOUT:-300}" \
          '--report-wipe-inventory' \
          '--ignore-controller-configmap app/kube-root-ca.crt' \
          '--ignore-controller-configmap app2/kube-root-ca.crt' \
          '--ignore-controller-conf-sync-secret edgion-system/edgion-conf-sync-ca' \
          '--ignore-controller-conf-sync-secret edgion-system/edgion-conf-sync-certs' \
          '--mode fail'; do
          echo "$args" | grep -q -- "$expected" \
            || { echo "missing validator argument: $expected" >&2; exit 9; }
        done
        case "${FAKE_VALIDATOR_MODE:-clean}" in
          clean) echo '[INFO] wipe-all: total=2 errors=0 duration_ms=1'; exit 0 ;;
          residue) echo 'Cache residue detected after 30s'; exit 1 ;;
          infrastructure) echo '[ERROR] collect failed'; exit 2 ;;
          unexpected) echo 'unexpected validator failure'; exit 7 ;;
          empty) exit 0 ;;
        esac
        ;;
      *)
        echo "unexpected kubectl exec: $*" >&2
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
export FAKE_KUBECTL_LOG="$CALL_LOG"
export INVENTORY_TIMEOUT_SECS=1

run_case() {
  local expected_rc="$1"
  local output_file="$2"
  local error_file="$3"
  shift 3
  set +e
  "$HELPER" "$@" > "$output_file" 2> "$error_file"
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
  local validator_rc="$2"
  local reason="$3"
  local output_file="$4"
  local failed=1
  if [ "$status" = "PASS" ]; then
    failed=0
  fi
  expected="@@FINAL_GATE_RESULT CacheResidue ${status} validator_rc=${validator_rc} reason=${reason}
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
unset FAKE_VALIDATOR_MODE FAKE_MISSING_BINARY FAKE_NO_CLIENT_POD FAKE_MISSING_ENDPOINTS FAKE_ROOT_CA_QUERY_FAIL FAKE_CONF_SYNC_SECRET_QUERY_FAIL
run_case 0 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol PASS 0 clean "$OUTPUT_FILE"

# A custom wipe deadline does not change the polling deadline.
export FAKE_EXPECTED_WIPE_TIMEOUT=420
run_case 0 "$OUTPUT_FILE" "$ERROR_FILE" --wipe-timeout-secs 420
assert_protocol PASS 0 clean "$OUTPUT_FILE"
unset FAKE_EXPECTED_WIPE_TIMEOUT

# Invalid deadlines fail before any cluster read or validator execution.
for value in 0 invalid; do
  : > "$CALL_LOG"
  export WIPE_TIMEOUT_SECS="$value"
  run_case 2 "$OUTPUT_FILE" "$ERROR_FILE"
  [ ! -s "$CALL_LOG" ]
  grep -q -- '--wipe-timeout-secs must be a positive integer' "$ERROR_FILE"
done
unset WIPE_TIMEOUT_SECS

export FAKE_VALIDATOR_MODE=residue
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 1 residue "$OUTPUT_FILE"
grep -q 'Cache residue detected' "$ERROR_FILE"

export FAKE_VALIDATOR_MODE=infrastructure
: > "$CALL_LOG"
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 2 infrastructure "$OUTPUT_FILE"
[ "$(grep -c -- ' -- /usr/local/bin/cache_residue ' "$CALL_LOG")" -eq 1 ]

export FAKE_VALIDATOR_MODE=unexpected
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 7 unexpected-rc "$OUTPUT_FILE"

export FAKE_VALIDATOR_MODE=empty
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL 0 empty-output "$OUTPUT_FILE"

unset FAKE_VALIDATOR_MODE
export FAKE_MISSING_BINARY=true
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL not-run missing-binary "$OUTPUT_FILE"

unset FAKE_MISSING_BINARY
export FAKE_ROOT_CA_QUERY_FAIL=true
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL not-run root-ca-inventory "$OUTPUT_FILE"

unset FAKE_ROOT_CA_QUERY_FAIL
export FAKE_CONF_SYNC_SECRET_QUERY_FAIL=true
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL not-run conf-sync-secret-inventory "$OUTPUT_FILE"

unset FAKE_CONF_SYNC_SECRET_QUERY_FAIL
export FAKE_NO_CLIENT_POD=true
run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
assert_protocol FAIL not-run missing-pod "$OUTPUT_FILE"

unset FAKE_NO_CLIENT_POD
for service in controller gateway; do
  export FAKE_MISSING_ENDPOINTS="$service"
  run_case 1 "$OUTPUT_FILE" "$ERROR_FILE"
  if [ "$service" = "controller" ]; then
    assert_protocol FAIL not-run missing-controller-endpoints "$OUTPUT_FILE"
  else
    assert_protocol FAIL not-run missing-gateway-endpoints "$OUTPUT_FILE"
  fi
done

echo "run_cache_residue_gate_test.sh: PASS"
