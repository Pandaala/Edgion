#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
K8S_SCRIPT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
TEST_TMP_DIR="$(mktemp -d /tmp/edgion-k8s-cleanup-test.XXXXXX)"
FAKE_BIN_DIR="${TEST_TMP_DIR}/bin"
DEPLOY_ROOT="${TEST_TMP_DIR}/kubernetes"
CONF_ROOT="${TEST_TMP_DIR}/conf"
OWNERSHIP_FILE="${TEST_TMP_DIR}/ownership.tsv"
CALL_LOG="${TEST_TMP_DIR}/kubectl.log"
UNOWNED_SENTINEL="${TEST_TMP_DIR}/generated/unowned"

cleanup() {
  rm -rf "${TEST_TMP_DIR}"
}
trap cleanup EXIT

mkdir -p "${FAKE_BIN_DIR}" "${DEPLOY_ROOT}" "${CONF_ROOT}" "$(dirname "${UNOWNED_SENTINEL}")"
printf 'apiVersion: v1\nkind: Namespace\nmetadata:\n  name: edgion-test\n' > "${DEPLOY_ROOT}/namespace.yaml"
cat > "${CONF_ROOT}/owned.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: public-gateway
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: edgion-controller
---
apiVersion: v1
kind: Namespace
metadata:
  name: app
EOF
printf 'unowned\n' > "${UNOWNED_SENTINEL}"
cat > "${OWNERSHIP_FILE}" <<'EOF'
cluster	GatewayClass	GatewayClass.gateway.networking.k8s.io	public-gateway
cluster	ClusterRole	ClusterRole.rbac.authorization.k8s.io	edgion-controller
namespace	Namespace	Namespace	app
namespace	Namespace	Namespace	edgion-test
EOF

cat > "${FAKE_BIN_DIR}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${FAKE_KUBECTL_LOG}"
if [[ "${FAKE_KUBECTL_MODE:-}" == "delete-failure" && "$*" == delete\ namespace\ * ]]; then
  exit 1
fi
if [[ "${FAKE_KUBECTL_MODE:-}" == "get-failure" && "${1:-}" == "get" ]]; then
  exit 1
fi
if [[ "${FAKE_KUBECTL_MODE:-}" == "first-crd-failure" \
  && "$*" == *"delete -f "*"/edgion-crd/"* ]]; then
  exit 1
fi
exit 0
EOF
chmod 755 "${FAKE_BIN_DIR}/kubectl"

export PATH="${FAKE_BIN_DIR}:${PATH}"
export FAKE_KUBECTL_LOG="${CALL_LOG}"
export K8S_DEPLOY_ROOT="${DEPLOY_ROOT}"
export CONF_ROOT
export GENERATED_DIR="${TEST_TMP_DIR}/generated"

"${K8S_SCRIPT_DIR}/cleanup.sh" --ownership-file "${OWNERSHIP_FILE}" \
  --skip-generated-artifacts --ns-timeout 1 >/dev/null

grep -Fq 'delete GatewayClass.gateway.networking.k8s.io public-gateway --ignore-not-found=true --wait=false' "${CALL_LOG}"
grep -Fq 'delete ClusterRole.rbac.authorization.k8s.io edgion-controller --ignore-not-found=true --wait=false' "${CALL_LOG}"
namespace_delete="$(grep '^delete namespace ' "${CALL_LOG}")"
if [[ " ${namespace_delete} " != *' app '* || " ${namespace_delete} " != *' edgion-test '* ]]; then
  echo "namespace deletion was not batched or missed owned namespaces: ${namespace_delete}" >&2
  exit 1
fi
if [[ " ${namespace_delete} " == *' other '* || " ${namespace_delete} " == *' kube-system '* ]]; then
  echo "namespace deletion escaped the ownership inventory: ${namespace_delete}" >&2
  exit 1
fi
if grep -Eq '(^| )-f( |$)|(^| )-k( |$)' "${CALL_LOG}"; then
  echo "cleanup regressed to manifest-by-manifest deletion" >&2
  cat "${CALL_LOG}" >&2
  exit 1
fi
if [[ ! -f "${UNOWNED_SENTINEL}" ]]; then
  echo "environment override escaped the generated-artifact boundary" >&2
  exit 1
fi

cat > "${TEST_TMP_DIR}/invalid-ownership.tsv" <<'EOF'
cluster	Namespace	Namespace	kube-system
EOF
if "${K8S_SCRIPT_DIR}/cleanup.sh" --ownership-file "${TEST_TMP_DIR}/invalid-ownership.tsv" \
  --skip-generated-artifacts --ns-timeout 1 >/dev/null 2>&1; then
  echo "invalid ownership tuple was accepted" >&2
  exit 1
fi

cat > "${TEST_TMP_DIR}/unowned-namespace.tsv" <<'EOF'
namespace	Namespace	Namespace	other
EOF
if "${K8S_SCRIPT_DIR}/cleanup.sh" --ownership-file "${TEST_TMP_DIR}/unowned-namespace.tsv" \
  --skip-generated-artifacts --ns-timeout 1 >/dev/null 2>&1; then
  echo "namespace absent from the current manifest inventory was accepted" >&2
  exit 1
fi

cat > "${TEST_TMP_DIR}/namespace-only.tsv" <<'EOF'
namespace	Namespace	Namespace	edgion-test
EOF
if FAKE_KUBECTL_MODE=delete-failure "${K8S_SCRIPT_DIR}/cleanup.sh" \
  --ownership-file "${TEST_TMP_DIR}/namespace-only.tsv" \
  --skip-generated-artifacts --ns-timeout 0 >/dev/null 2>&1; then
  echo "namespace delete failure was swallowed" >&2
  exit 1
fi
if FAKE_KUBECTL_MODE=get-failure "${K8S_SCRIPT_DIR}/cleanup.sh" \
  --ownership-file "${TEST_TMP_DIR}/namespace-only.tsv" \
  --skip-generated-artifacts --ns-timeout 0 >/dev/null 2>&1; then
  echo "namespace verification API failure was swallowed" >&2
  exit 1
fi
if FAKE_KUBECTL_MODE=first-crd-failure "${K8S_SCRIPT_DIR}/cleanup.sh" \
  --ownership-file "${TEST_TMP_DIR}/namespace-only.tsv" \
  --skip-generated-artifacts --with-crds --ns-timeout 0 >/dev/null 2>&1; then
  echo "first CRD delete failure was swallowed" >&2
  exit 1
fi

echo "cleanup_test.sh: PASS"
