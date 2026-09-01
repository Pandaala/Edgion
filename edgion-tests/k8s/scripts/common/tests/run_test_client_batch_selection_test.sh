#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMMON_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEST_TMP_DIR="$(mktemp -d /tmp/edgion-test-client-batch-selection.XXXXXX)"
FAKE_BIN_DIR="${TEST_TMP_DIR}/bin"
INNER_SCRIPT="${TEST_TMP_DIR}/inner.sh"
SUITES_FILE="${TEST_TMP_DIR}/suites.txt"

cleanup() {
  rm -rf "${TEST_TMP_DIR}"
}
trap cleanup EXIT

mkdir -p "${FAKE_BIN_DIR}"
cat > "${SUITES_FILE}" <<'EOF'
EdgionPlugins|BodyPlugins
EdgionPlugins|Guardrail
EOF

cat > "${FAKE_BIN_DIR}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

if [ "${1:-}" = get ] && [ "${2:-}" = pods ]; then
  printf 'test-client-pod <none>\n'
  exit 0
fi
if [ "${1:-}" = get ] && [ "${2:-}" = --raw ]; then
  case "$*" in
    *service-name%3Dedgion-test-server*)
      cat <<'JSON'
{"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","items":[{"metadata":{"name":"test-server"},"addressType":"IPv4","ports":[{"name":"http","protocol":"TCP","port":30001}],"endpoints":[{"addresses":["10.0.0.1"],"conditions":{"ready":true}},{"addresses":["10.0.0.2"],"conditions":{"ready":true}}]}]}
JSON
      ;;
    *service-name%3Dedgion-gateway*)
      cat <<'JSON'
{"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","items":[{"metadata":{"name":"gateway"},"addressType":"IPv4","ports":[{"name":"metrics-12090","protocol":"TCP","port":12090}],"endpoints":[{"addresses":["10.0.0.9"],"conditions":{"ready":true}}]}]}
JSON
      ;;
    *)
      echo "unexpected EndpointSlice query: $*" >&2
      exit 9
      ;;
  esac
  exit 0
fi
if [ "${1:-}" = exec ] && [ "${2:-}" = -i ]; then
  cat > "${FAKE_INNER_SCRIPT}"
  printf '@@BATCH_SUMMARY total=2 failed=0\n'
  exit 0
fi

echo "unexpected kubectl arguments: $*" >&2
exit 9
EOF
chmod 755 "${FAKE_BIN_DIR}/kubectl"

export PATH="${FAKE_BIN_DIR}:${PATH}"
export FAKE_INNER_SCRIPT="${INNER_SCRIPT}"
export EDGION_TEST_GATEWAY_CAPABILITIES='tls=boringssl'
export EDGION_TEST_ADMIN_TOKEN='controller-token'
export EDGION_GATEWAY_ADMIN_TOKEN='gateway-token'

output="$("${COMMON_DIR}/run_test_client_batch.sh" "${SUITES_FILE}")"
case "$output" in
  *'Test server endpoints: 10.0.0.1,10.0.0.2'*) ;;
  *)
    echo "batch runner did not inventory every test-server endpoint" >&2
    echo "$output" >&2
    exit 1
    ;;
esac
grep -Fq 'export EDGION_TEST_BACKEND_ENDPOINT_IPS="10.0.0.1,10.0.0.2"' "${INNER_SCRIPT}"
grep -Fq -- "-i 'BodyPlugins'" "${INNER_SCRIPT}"
grep -Fq -- "-i 'Guardrail'" "${INNER_SCRIPT}"

echo "run_test_client_batch_selection_test.sh: PASS"
