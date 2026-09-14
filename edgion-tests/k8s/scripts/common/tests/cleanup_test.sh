#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
K8S_SCRIPT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
TEST_TMP_DIR="$(mktemp -d /tmp/edgion-k8s-cleanup-test.XXXXXX)"
FAKE_BIN_DIR="${TEST_TMP_DIR}/bin"
DEPLOY_ROOT="${TEST_TMP_DIR}/kubernetes"
CONF_ROOT="${TEST_TMP_DIR}/conf"
RUN_DIR="${TEST_TMP_DIR}/run"
OWNERSHIP_FILE="${RUN_DIR}/owned-cluster-keys.tsv"
CALL_LOG="${TEST_TMP_DIR}/kubectl.log"
GENERATED_SENTINEL="${RUN_DIR}/generated/owned"
UNOWNED_SENTINEL="${TEST_TMP_DIR}/outside-run"
EXPECTED_CONTEXT="test-context"

cleanup() {
  rm -rf "${TEST_TMP_DIR}"
}
trap cleanup EXIT

mkdir -p "${FAKE_BIN_DIR}" "${DEPLOY_ROOT}" "${CONF_ROOT}" \
  "$(dirname "${GENERATED_SENTINEL}")"
printf 'apiVersion: v1\nkind: Namespace\nmetadata:\n  name: edgion-test\n' > "${DEPLOY_ROOT}/namespace.yaml"
cat > "${CONF_ROOT}/owned.yaml" <<'EOF'
apiVersion: edgion.io/v1
kind: EdgionGatewayConfig
metadata:
  name: example-gateway
---
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
---
apiVersion: v1
kind: Namespace
metadata:
  name: edgion-system
EOF
printf 'owned\n' > "${GENERATED_SENTINEL}"
printf 'unowned\n' > "${UNOWNED_SENTINEL}"
cat > "${OWNERSHIP_FILE}" <<'EOF'
cluster	EdgionGatewayConfig	EdgionGatewayConfig.edgion.io	example-gateway
cluster	GatewayClass	GatewayClass.gateway.networking.k8s.io	public-gateway
namespace	Namespace	Namespace	app
namespace	Namespace	Namespace	edgion-test
EOF

cat > "${FAKE_BIN_DIR}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${FAKE_KUBECTL_LOG}"
if [[ "$*" == "config current-context" ]]; then
  printf '%s\n' "${FAKE_CURRENT_CONTEXT:-test-context}"
  exit 0
fi
if [[ "${1:-}" == "--context" ]]; then
  [[ "${2:-}" == "test-context" ]] || exit 1
  shift 2
fi
if [[ "${FAKE_KUBECTL_MODE:-}" == "delete-failure" && "$*" == delete\ namespace\ * ]]; then
  exit 1
fi
if [[ "${FAKE_KUBECTL_MODE:-}" == "get-failure" && "${1:-}" == "get" ]]; then
  exit 1
fi
if [[ "${FAKE_KUBECTL_MODE:-}" == "retained-egc" \
  && "${1:-}" == "get" && "${2:-}" == "EdgionGatewayConfig.edgion.io" ]]; then
  printf '%s\n' 'edgiongatewayconfig.edgion.io/example-gateway'
  exit 0
fi
exit 0
EOF
chmod 755 "${FAKE_BIN_DIR}/kubectl"
cat > "${FAKE_BIN_DIR}/docker" <<'EOF'
#!/usr/bin/env bash
echo "integration cleanup invoked docker" >&2
exit 1
EOF
chmod 755 "${FAKE_BIN_DIR}/docker"

export PATH="${FAKE_BIN_DIR}:${PATH}"
export FAKE_KUBECTL_LOG="${CALL_LOG}"
export K8S_DEPLOY_ROOT="${DEPLOY_ROOT}"
export CONF_ROOT

canonical_inventory="$(
  "${K8S_SCRIPT_DIR}/common/write_cleanup_inventory.sh" "${CONF_ROOT}" "${DEPLOY_ROOT}"
)"
if ! grep -Fq $'cluster\tEdgionGatewayConfig\tEdgionGatewayConfig.edgion.io\texample-gateway' \
  <<< "${canonical_inventory}" \
  || grep -Eq 'edgion-system|dragonfly-system|ClusterRole' <<< "${canonical_inventory}"; then
  echo "canonical cleanup inventory crossed the installation ownership boundary" >&2
  exit 1
fi

help_output="$("${K8S_SCRIPT_DIR}/cleanup.sh" --help)"
if grep -Fq -- '--with-crds' <<< "${help_output}" \
  || grep -Fq -- '--with-images' <<< "${help_output}" \
  || ! grep -Fq 'CRDs, installation namespaces/RBAC, and local images are never deleted' \
    <<< "${help_output}"; then
  echo "cleanup help did not preserve shared infrastructure boundaries" >&2
  exit 1
fi

"${K8S_SCRIPT_DIR}/cleanup.sh" --ownership-file "${OWNERSHIP_FILE}" \
  --expected-context "${EXPECTED_CONTEXT}" --ns-timeout 1 >/dev/null

grep -Fq -- '--context test-context delete GatewayClass.gateway.networking.k8s.io public-gateway --ignore-not-found=true --wait=false' "${CALL_LOG}"
grep -Fq -- '--context test-context delete EdgionGatewayConfig.edgion.io example-gateway --ignore-not-found=true --wait=false' "${CALL_LOG}"
namespace_delete="$(grep -- '--context test-context delete namespace ' "${CALL_LOG}")"
if [[ " ${namespace_delete} " != *' app '* || " ${namespace_delete} " != *' edgion-test '* ]]; then
  echo "namespace deletion was not batched or missed owned namespaces: ${namespace_delete}" >&2
  exit 1
fi
if grep -Eq 'delete (namespace )?.*(edgion-system|dragonfly-system)|delete ClusterRole' "${CALL_LOG}"; then
  echo "cleanup deleted installation-scoped infrastructure" >&2
  cat "${CALL_LOG}" >&2
  exit 1
fi
if grep -Eq '(^| )-f( |$)|(^| )-k( |$)|docker' "${CALL_LOG}"; then
  echo "cleanup escaped exact object deletion" >&2
  cat "${CALL_LOG}" >&2
  exit 1
fi
if [[ -e "${GENERATED_SENTINEL}" || ! -f "${UNOWNED_SENTINEL}" ]]; then
  echo "generated-artifact cleanup escaped its run directory" >&2
  exit 1
fi

: > "${CALL_LOG}"
if FAKE_KUBECTL_MODE=retained-egc "${K8S_SCRIPT_DIR}/cleanup.sh" \
  --ownership-file "${OWNERSHIP_FILE}" --expected-context "${EXPECTED_CONTEXT}" \
  --ns-timeout 0 >/dev/null 2>&1; then
  echo "a retained cluster-scoped custom resource passed cleanup verification" >&2
  exit 1
fi

for removed_option in --with-crds --with-images; do
  if "${K8S_SCRIPT_DIR}/cleanup.sh" --ownership-file "${OWNERSHIP_FILE}" \
    --expected-context "${EXPECTED_CONTEXT}" "${removed_option}" >/dev/null 2>&1; then
    echo "removed option was still accepted: ${removed_option}" >&2
    exit 1
  fi
done
if "${K8S_SCRIPT_DIR}/cleanup.sh" --expected-context "${EXPECTED_CONTEXT}" >/dev/null 2>&1; then
  echo "cleanup inferred ownership without a run inventory" >&2
  exit 1
fi
if FAKE_CURRENT_CONTEXT=other-context "${K8S_SCRIPT_DIR}/cleanup.sh" \
  --ownership-file "${OWNERSHIP_FILE}" --expected-context "${EXPECTED_CONTEXT}" \
  >/dev/null 2>&1; then
  echo "cleanup accepted the wrong kubectl context" >&2
  exit 1
fi

make_inventory() {
  local case_dir="$1" content="$2"
  mkdir -p "${case_dir}"
  printf '%s\n' "${content}" > "${case_dir}/owned-cluster-keys.tsv"
}

make_inventory "${TEST_TMP_DIR}/invalid" $'cluster\tNamespace\tNamespace\tkube-system'
if "${K8S_SCRIPT_DIR}/cleanup.sh" \
  --ownership-file "${TEST_TMP_DIR}/invalid/owned-cluster-keys.tsv" \
  --expected-context "${EXPECTED_CONTEXT}" --ns-timeout 1 >/dev/null 2>&1; then
  echo "invalid ownership tuple was accepted" >&2
  exit 1
fi

make_inventory "${TEST_TMP_DIR}/unowned" $'namespace\tNamespace\tNamespace\tother'
if "${K8S_SCRIPT_DIR}/cleanup.sh" \
  --ownership-file "${TEST_TMP_DIR}/unowned/owned-cluster-keys.tsv" \
  --expected-context "${EXPECTED_CONTEXT}" --ns-timeout 1 >/dev/null 2>&1; then
  echo "namespace absent from the current manifest inventory was accepted" >&2
  exit 1
fi

make_inventory "${TEST_TMP_DIR}/namespace-only" $'namespace\tNamespace\tNamespace\tedgion-test'
if FAKE_KUBECTL_MODE=delete-failure "${K8S_SCRIPT_DIR}/cleanup.sh" \
  --ownership-file "${TEST_TMP_DIR}/namespace-only/owned-cluster-keys.tsv" \
  --expected-context "${EXPECTED_CONTEXT}" --ns-timeout 0 >/dev/null 2>&1; then
  echo "namespace delete failure was swallowed" >&2
  exit 1
fi
if FAKE_KUBECTL_MODE=get-failure "${K8S_SCRIPT_DIR}/cleanup.sh" \
  --ownership-file "${TEST_TMP_DIR}/namespace-only/owned-cluster-keys.tsv" \
  --expected-context "${EXPECTED_CONTEXT}" --ns-timeout 0 >/dev/null 2>&1; then
  echo "namespace verification API failure was swallowed" >&2
  exit 1
fi

echo "cleanup_test.sh: PASS"
