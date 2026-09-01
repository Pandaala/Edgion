#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../verify_plugin_validation_admission.sh"
TEST_ROOT="$(mktemp -d /tmp/edgion-plugin-admission-test.XXXXXX)"
FAKE_BIN="${TEST_ROOT}/bin"
INVALID_DIR="${TEST_ROOT}/invalid"
CALL_LOG="${TEST_ROOT}/kubectl.log"

cleanup() {
  rm -rf "${TEST_ROOT}"
}
trap cleanup EXIT INT TERM

mkdir -p "${FAKE_BIN}" "${INVALID_DIR}"
for fixture in allof_to_anyof anyof_to_anyof both_operators; do
  : > "${INVALID_DIR}/${fixture}.yaml"
done

cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >> "${FAKE_KUBECTL_LOG}"
fixture="$(basename "${*: -1}")"
if [ "${FAKE_ACCEPT_FIXTURE:-}" = "${fixture}" ]; then
  echo "dry-run accepted"
  exit 0
fi
if [ "${FAKE_UNEXPECTED_FIXTURE:-}" = "${fixture}" ]; then
  echo "connection refused" >&2
  exit 1
fi
echo "strict decoding error: unknown field" >&2
exit 1
EOF
chmod 755 "${FAKE_BIN}/kubectl"

export PATH="${FAKE_BIN}:${PATH}"
export FAKE_KUBECTL_LOG="${CALL_LOG}"
export PLUGIN_VALIDATION_INVALID_DIR="${INVALID_DIR}"

run_case() {
  local expected_rc="$1"
  local expected_message="$2"
  local output_file="${TEST_ROOT}/output"
  set +e
  "${HELPER}" > "${output_file}" 2>&1
  local actual_rc=$?
  set -e
  if [ "${actual_rc}" -ne "${expected_rc}" ]; then
    echo "Expected rc=${expected_rc}, got rc=${actual_rc}" >&2
    cat "${output_file}" >&2
    exit 1
  fi
  if ! grep -qF "${expected_message}" "${output_file}"; then
    echo "Missing expected message: ${expected_message}" >&2
    cat "${output_file}" >&2
    exit 1
  fi
}

: > "${CALL_LOG}"
unset FAKE_ACCEPT_FIXTURE FAKE_UNEXPECTED_FIXTURE
run_case 0 "PluginValidationStatus admission preflight passed"
[ "$(wc -l < "${CALL_LOG}" | tr -d ' ')" -eq 3 ]

export FAKE_ACCEPT_FIXTURE="anyof_to_anyof.yaml"
run_case 1 "Kubernetes admission unexpectedly accepted"

unset FAKE_ACCEPT_FIXTURE
export FAKE_UNEXPECTED_FIXTURE="both_operators.yaml"
run_case 1 "Kubernetes admission failed for an unexpected reason"

unset FAKE_UNEXPECTED_FIXTURE
rm "${INVALID_DIR}/both_operators.yaml"
run_case 1 "Expected 3 malformed ConditionSet fixtures, found 2"

echo "verify_plugin_validation_admission_test.sh: PASS"
