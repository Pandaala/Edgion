#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LEASE_SCRIPT="${SCRIPT_DIR}/../cluster_lease.sh"
TEST_TMP_DIR="$(mktemp -d /tmp/edgion-k8s-cluster-lease-test.XXXXXX)"
FAKE_BIN_DIR="${TEST_TMP_DIR}/bin"
CALL_LOG="${TEST_TMP_DIR}/kubectl.log"

cleanup() {
  rm -rf "${TEST_TMP_DIR}"
}
trap cleanup EXIT

mkdir -p "${FAKE_BIN_DIR}"
cat > "${FAKE_BIN_DIR}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${FAKE_KUBECTL_LOG}"
[[ "${1:-}" == "--context" && "${2:-}" == "test-context" ]] || exit 1
shift 2

if [[ "${1:-}" == "create" ]]; then
  cat >/dev/null
  [[ "${FAKE_LEASE_MODE:-success}" != "conflict" ]]
  exit
fi
if [[ "$*" == *"get lease edgion-k8s-integration"* ]]; then
  case "${FAKE_LEASE_MODE:-success}" in
    missing) exit 0 ;;
    mismatch|conflict) printf '%s' 'other-run' ;;
    *) printf '%s' 'run-1' ;;
  esac
  exit 0
fi
if [[ "$*" == *"delete lease edgion-k8s-integration --wait=true"* ]]; then
  exit 0
fi
exit 1
EOF
chmod 755 "${FAKE_BIN_DIR}/kubectl"

export PATH="${FAKE_BIN_DIR}:${PATH}"
export FAKE_KUBECTL_LOG="${CALL_LOG}"

# shellcheck source=../cluster_lease.sh
source "${LEASE_SCRIPT}"

acquire_k8s_integration_lease test-context default edgion-k8s-integration run-1
grep -Fq -- '--context test-context create -f -' "${CALL_LOG}"

if FAKE_LEASE_MODE=conflict acquire_k8s_integration_lease \
  test-context default edgion-k8s-integration run-1 >/dev/null 2>&1; then
  echo "an existing cluster lease was overwritten" >&2
  exit 1
fi

: > "${CALL_LOG}"
release_k8s_integration_lease test-context default edgion-k8s-integration run-1
grep -Fq -- '--context test-context -n default delete lease edgion-k8s-integration --wait=true' \
  "${CALL_LOG}"

: > "${CALL_LOG}"
if FAKE_LEASE_MODE=mismatch release_k8s_integration_lease \
  test-context default edgion-k8s-integration run-1 >/dev/null 2>&1; then
  echo "a lease owned by another run was released" >&2
  exit 1
fi
if grep -Fq 'delete lease' "${CALL_LOG}"; then
  echo "mismatched lease release reached delete" >&2
  exit 1
fi

: > "${CALL_LOG}"
FAKE_LEASE_MODE=missing release_k8s_integration_lease \
  test-context default edgion-k8s-integration run-1
if grep -Fq 'delete lease' "${CALL_LOG}"; then
  echo "missing lease triggered deletion" >&2
  exit 1
fi

echo "cluster_lease_test.sh: PASS"
