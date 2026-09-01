#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INVENTORY_SCRIPT="${SCRIPT_DIR}/endpoint_inventory.sh"
TEST_TMP_DIR="$(mktemp -d /tmp/edgion-endpoint-inventory-test.XXXXXX)"
FAKE_BIN_DIR="${TEST_TMP_DIR}/bin"
FIXTURE_FILE="${TEST_TMP_DIR}/fixture.json"

cleanup() {
  rm -rf "$TEST_TMP_DIR"
}
trap cleanup EXIT

mkdir -p "$FAKE_BIN_DIR"

cat > "${FAKE_BIN_DIR}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

expected_prefix='get --raw /apis/discovery.k8s.io/v1/namespaces/test-ns/endpointslices?labelSelector=kubernetes.io%2Fservice-name%3Dtest-service --request-timeout '
actual="$*"
case "$actual" in
  "${expected_prefix}"[1-9]*s) ;;
  "get deployment edgion-test-server --namespace edgion-test --output jsonpath="*)
    printf '%s\n' "${FAKE_DEPLOYMENT_REPLICAS:?}"
    exit 0
    ;;
  *)
    echo "unexpected kubectl arguments: $actual" >&2
    exit 9
    ;;
esac
cat "$FAKE_KUBECTL_FIXTURE"
EOF
chmod 755 "${FAKE_BIN_DIR}/kubectl"

export PATH="${FAKE_BIN_DIR}:${PATH}"
export FAKE_KUBECTL_FIXTURE="$FIXTURE_FILE"

pass_count=0

pass() {
  pass_count=$((pass_count + 1))
  echo "[PASS] $1"
}

fail() {
  echo "[FAIL] $1" >&2
  exit 1
}

assert_eq() {
  local expected="$1"
  local actual="$2"
  local label="$3"
  if [ "$actual" != "$expected" ]; then
    echo "Expected: $expected" >&2
    echo "Actual:   $actual" >&2
    fail "$label"
  fi
  pass "$label"
}

assert_failure_contains() {
  local expected_text="$1"
  local label="$2"
  shift 2
  local output rc
  set +e
  output=$("$@" 2>&1)
  rc=$?
  set -e
  if [ "$rc" -eq 0 ]; then
    fail "$label returned success"
  fi
  case "$output" in
    *"$expected_text"*) pass "$label" ;;
    *)
      echo "$output" >&2
      fail "$label did not report: $expected_text"
      ;;
  esac
}

write_fixture() {
  printf '%s\n' "$1" > "$FIXTURE_FILE"
}

write_fixture '{
  "apiVersion": "discovery.k8s.io/v1",
  "kind": "EndpointSliceList",
  "items": [
    {
      "metadata": {"name": "slice-b"},
      "addressType": "IPv4",
      "ports": [{"name": "http", "protocol": "TCP", "port": 30001}],
      "endpoints": [
        {"addresses": ["10.0.0.3"], "conditions": {"ready": true}},
        {"addresses": ["10.0.0.9"], "conditions": {"ready": false}},
        {"addresses": ["10.0.0.8"], "conditions": {"ready": true, "terminating": true}}
      ]
    },
    {
      "metadata": {"name": "slice-a"},
      "addressType": "IPv4",
      "ports": [{"name": "http", "protocol": "TCP", "port": 30001}],
      "endpoints": [
        {"addresses": ["10.0.0.1"], "conditions": {}},
        {"addresses": ["10.0.0.2"], "conditions": {"ready": true, "terminating": false}},
        {"addresses": ["10.0.0.3"], "conditions": {"ready": true}}
      ]
    }
  ]
}'

json_output=$("$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http --expected-count 3 --timeout-secs 0)
assert_eq '[{"address":"10.0.0.1","port":30001,"portName":"http","protocol":"TCP","addressType":"IPv4"},{"address":"10.0.0.2","port":30001,"portName":"http","protocol":"TCP","addressType":"IPv4"},{"address":"10.0.0.3","port":30001,"portName":"http","protocol":"TCP","addressType":"IPv4"}]' "$json_output" "stable JSON treats unknown readiness as ready and filters not-ready, terminating, and duplicate addresses"

ip_output=$("$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port 30001 --expected-count 3 --timeout-secs 0 --output ips)
assert_eq '10.0.0.1,10.0.0.2,10.0.0.3' "$ip_output" "numeric port selector and comma-separated IP output"

write_fixture '{
  "apiVersion": "discovery.k8s.io/v1",
  "kind": "EndpointSliceList",
  "items": [{
    "metadata": {"name": "missing-port"},
    "addressType": "IPv4",
    "ports": [{"name": "grpc", "protocol": "TCP", "port": 30021}],
    "endpoints": [{"addresses": ["10.0.0.2"], "conditions": {"ready": true}}]
  }]
}'
assert_failure_contains 'no EndpointSlice port matches http' "missing selected port fails" \
  "$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http --timeout-secs 0
empty_output=$("$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http \
  --expected-count 0 --timeout-secs 0 --output hostports)
assert_eq '' "$empty_output" "zero expected endpoints accepts an absent EndpointSlice port"

write_fixture '{}'
assert_failure_contains 'not valid discovery.k8s.io/v1 JSON' "empty object cannot satisfy zero expected endpoints" \
  "$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http \
    --expected-count 0 --timeout-secs 0

write_fixture '{
  "apiVersion": "v1",
  "kind": "List",
  "items": []
}'
assert_failure_contains 'not valid discovery.k8s.io/v1 JSON' "wrong list kind cannot satisfy zero expected endpoints" \
  "$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http \
    --expected-count 0 --timeout-secs 0

write_fixture '{
  "apiVersion": "discovery.k8s.io/v1beta1",
  "kind": "EndpointSliceList",
  "items": []
}'
assert_failure_contains 'not valid discovery.k8s.io/v1 JSON' "wrong API version cannot satisfy zero expected endpoints" \
  "$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http \
    --expected-count 0 --timeout-secs 0

write_fixture '{
  "apiVersion": "discovery.k8s.io/v1",
  "kind": "EndpointSliceList",
  "items": [{
    "metadata": {"name": "invalid-ready-endpoint"},
    "addressType": "IPv4",
    "ports": [{"name": "http", "protocol": "TCP", "port": 70000}],
    "endpoints": [{"addresses": ["not-an-ip"], "conditions": {"ready": true}}]
  }]
}'
assert_failure_contains 'invalid selected port' "selected port value is validated" \
  "$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http --timeout-secs 0

write_fixture '{
  "apiVersion": "discovery.k8s.io/v1",
  "kind": "EndpointSliceList",
  "items": [{
    "metadata": {"name": "invalid-ready-endpoint"},
    "addressType": "IPv4",
    "ports": [{"name": "http", "protocol": "TCP", "port": 30001}],
    "endpoints": [{"addresses": ["not-an-ip"], "conditions": {"ready": true}}]
  }]
}'
assert_failure_contains 'invalid ready endpoint address' "ready endpoint address is validated" \
  "$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http --timeout-secs 0

write_fixture '{
  "apiVersion": "discovery.k8s.io/v1",
  "kind": "EndpointSliceList",
  "items": [{
    "metadata": {"name": "count-mismatch"},
    "addressType": "IPv4",
    "ports": [{"name": "http", "protocol": "TCP", "port": 30001}],
    "endpoints": [{"addresses": ["10.0.0.2"], "conditions": {"ready": true}}]
  }]
}'
count_started=$(date +%s)
assert_failure_contains 'expected 2 endpoints but found 1' "expected count uses bounded timeout" \
  "$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http --expected-count 2 --timeout-secs 1 --interval-secs 1
count_elapsed=$(( $(date +%s) - count_started ))
if [ "$count_elapsed" -gt 3 ]; then
  fail "expected count timeout exceeded its bound"
fi
pass "expected count timeout stayed bounded"

write_fixture '{
  "apiVersion": "discovery.k8s.io/v1",
  "kind": "EndpointSliceList",
  "items": [{
    "metadata": {"name": "invalid-ipv6"},
    "addressType": "IPv6",
    "ports": [{"name": "http", "protocol": "TCP", "port": 30001}],
    "endpoints": [{"addresses": ["2001:::1"], "conditions": {"ready": true}}]
  }]
}'
assert_failure_contains 'invalid ready endpoint address' "malformed IPv6 address is rejected" \
  "$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http --timeout-secs 0

write_fixture '{
  "apiVersion": "discovery.k8s.io/v1",
  "kind": "EndpointSliceList",
  "items": [{
    "metadata": {"name": "ipv6"},
    "addressType": "IPv6",
    "ports": [{"name": "http", "protocol": "TCP", "port": 30001}],
    "endpoints": [
      {"addresses": ["2001:db8::2"], "conditions": {"ready": true}},
      {"addresses": ["2001:db8::1"], "conditions": {"ready": true}}
    ]
  }]
}'
hostport_output=$("$INVENTORY_SCRIPT" --namespace test-ns --service test-service --port http --expected-count 2 --timeout-secs 0 --output hostports)
assert_eq '[2001:db8::1]:30001,[2001:db8::2]:30001' "$hostport_output" "IPv6 hostports are bracketed and stable-sorted"

# shellcheck source=endpoint_inventory.sh
source "$INVENTORY_SCRIPT"
export FAKE_DEPLOYMENT_REPLICAS='3 3'
replica_count=$(endpoint_inventory_strict_deployment_replicas edgion-test edgion-test-server)
assert_eq '3' "$replica_count" "available Deployment replicas provide the strict endpoint count"
export FAKE_DEPLOYMENT_REPLICAS='3 2'
assert_failure_contains 'has 2 available replicas, expected 3' "partially available Deployment is rejected" \
  endpoint_inventory_strict_deployment_replicas edgion-test edgion-test-server
export FAKE_DEPLOYMENT_REPLICAS='3'
assert_failure_contains 'available replica count must be a non-negative integer' \
  "missing available replica status is rejected" \
  endpoint_inventory_strict_deployment_replicas edgion-test edgion-test-server

echo "Endpoint inventory focused tests passed: ${pass_count}"
