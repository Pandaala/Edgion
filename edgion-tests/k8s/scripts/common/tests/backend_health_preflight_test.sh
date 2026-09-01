#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMMON_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEST_TMP_DIR="$(mktemp -d /tmp/edgion-backend-health-preflight-test.XXXXXX)"
FAKE_BIN_DIR="${TEST_TMP_DIR}/bin"
ATTEMPT_FILE="${TEST_TMP_DIR}/attempts"

cleanup() {
  rm -rf "${TEST_TMP_DIR}"
}
trap cleanup EXIT

mkdir -p "${FAKE_BIN_DIR}"
printf '0\n' > "${ATTEMPT_FILE}"

cat > "${FAKE_BIN_DIR}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

if [ "${1:-}" = get ] && [ "${2:-}" = --raw ]; then
  cat <<'JSON'
{"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","items":[{"metadata":{"name":"generated"},"addressType":"IPv4","ports":[{"name":"http","protocol":"TCP","port":30001}],"endpoints":[{"addresses":["10.0.0.1"],"conditions":{"ready":true}},{"addresses":["10.0.0.2"],"conditions":{"ready":true}}]}]}
JSON
  exit 0
fi

if [ "${1:-}" = --request-timeout=5s ] && [ "${2:-}" = exec ]; then
  endpoint="${*: -1}"
  attempts=$(cat "${FAKE_ATTEMPT_FILE}")
  attempts=$((attempts + 1))
  printf '%s\n' "$attempts" > "${FAKE_ATTEMPT_FILE}"
  if [ "$endpoint" = http://10.0.0.2:30001/health ] && [ "$attempts" -lt 4 ]; then
    echo 'temporary network failure' >&2
    exit 7
  fi
  printf 'ok\n'
  exit 0
fi

echo "unexpected kubectl arguments: $*" >&2
exit 9
EOF
chmod 755 "${FAKE_BIN_DIR}/kubectl"

export PATH="${FAKE_BIN_DIR}:${PATH}"
export FAKE_ATTEMPT_FILE="${ATTEMPT_FILE}"

# shellcheck source=../endpoint_inventory.sh
source "${COMMON_DIR}/endpoint_inventory.sh"

output="$(endpoint_inventory_wait_for_backend_health \
  test-ns test-service http 2 client-ns client-pod test-client 5)"
if [ "$output" != '10.0.0.1:30001,10.0.0.2:30001' ]; then
  echo "unexpected healthy endpoint output: $output" >&2
  exit 1
fi
if [ "$(cat "${ATTEMPT_FILE}")" -lt 4 ]; then
  echo "transient failure was not retried" >&2
  exit 1
fi

printf 'Backend health preflight focused test passed\n'
