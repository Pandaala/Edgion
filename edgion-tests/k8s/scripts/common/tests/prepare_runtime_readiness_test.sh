#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_RUNNER="${SCRIPT_DIR}/../../run_k8s_integration.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-k8s-prepare-readiness-test.XXXXXX)"
READINESS_FUNCTION_FILE="${TEMP_DIR}/readiness-functions.sh"
DECISION_FUNCTION_FILE="${TEMP_DIR}/decision-function.sh"
FAKE_BIN="${TEMP_DIR}/bin"
KUBECTL_LOG="${TEMP_DIR}/kubectl.log"

cleanup() {
  rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT INT TERM

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

awk '
  /^verify_service_manifest_ports\(\) \{/ { emit = 1 }
  /^verify_skip_prepare_preconditions\(\) \{/ { emit = 0 }
  emit { print }
' "${MAIN_RUNNER}" > "${READINESS_FUNCTION_FILE}"
awk '
  /^should_run_prepare\(\) \{/ { emit = 1 }
  /^preflight_kubernetes_cluster$/ { emit = 0 }
  emit { print }
' "${MAIN_RUNNER}" > "${DECISION_FUNCTION_FILE}"
grep -Fq 'verify_prepared_runtime_readiness()' "${READINESS_FUNCTION_FILE}" \
  || fail "could not extract runtime readiness functions"
grep -Fq 'should_run_prepare()' "${DECISION_FUNCTION_FILE}" \
  || fail "could not extract prepare decision function"

mkdir -p "${FAKE_BIN}" "${TEMP_DIR}/deploy/gateway" "${TEMP_DIR}/deploy/test/test-server"
cat > "${TEMP_DIR}/deploy/gateway/service.yaml" <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: edgion-gateway
spec:
  ports:
    - name: metrics
      port: 12090
      targetPort: 12090
      protocol: TCP
EOF
cat > "${TEMP_DIR}/deploy/test/test-server/service.yaml" <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: edgion-test-server
spec:
  ports:
    - name: http
      port: 30001
      targetPort: 30001
      protocol: TCP
EOF
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${KUBECTL_LOG}"
if [[ -n "${FAKE_KUBECTL_FAILURE_MATCH:-}" && "$*" == *"${FAKE_KUBECTL_FAILURE_MATCH}"* ]]; then
  exit 1
fi
case "$*" in
  *"get service edgion-gateway -o json"*)
    if [[ "${FAKE_MISSING_PORTS:-false}" == "true" ]]; then
      printf '{"spec":{"ports":[]}}\n'
    else
      printf '{"spec":{"ports":[{"name":"metrics","port":12090,"targetPort":12090,"protocol":"TCP"}]}}\n'
    fi
    ;;
  *"get service edgion-test-server -o json"*)
    printf '{"spec":{"ports":[{"name":"http","port":30001,"targetPort":30001,"protocol":"TCP"}]}}\n'
    ;;
esac
EOF
cat > "${TEMP_DIR}/endpoint-inventory.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'endpoint:%s\n' "$*" >> "${KUBECTL_LOG}"
[[ -z "${FAKE_ENDPOINT_FAILURE_MATCH:-}" || "$*" != *"${FAKE_ENDPOINT_FAILURE_MATCH}"* ]]
EOF
cat > "${TEMP_DIR}/test-client-peer-identity.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "assert" ]]
EOF
chmod +x \
  "${FAKE_BIN}/kubectl" \
  "${TEMP_DIR}/endpoint-inventory.sh" \
  "${TEMP_DIR}/test-client-peer-identity.sh"

K8S_DEPLOY_ROOT="${TEMP_DIR}/deploy"
ENDPOINT_INVENTORY="${TEMP_DIR}/endpoint-inventory.sh"
TEST_CLIENT_PEER_IDENTITY="${TEMP_DIR}/test-client-peer-identity.sh"
BACKEND_TEST_NAMESPACE=edgion-backend
TEST_SERVER_REPLICAS=3
export KUBECTL_LOG

# shellcheck source=/dev/null
source "${READINESS_FUNCTION_FILE}"

: > "${KUBECTL_LOG}"
PATH="${FAKE_BIN}:${PATH}" verify_prepared_runtime_readiness \
  || fail "complete prepared runtime was rejected"
[[ "$(grep -c '^rollout status ' "${KUBECTL_LOG}")" -eq 9 ]] \
  || fail "prepared runtime did not check every deployment rollout"
[[ "$(grep -c '^endpoint:' "${KUBECTL_LOG}")" -eq 3 ]] \
  || fail "prepared runtime did not check every required EndpointSlice inventory"

if PATH="${FAKE_BIN}:${PATH}" FAKE_KUBECTL_FAILURE_MATCH='deployment/edgion-controller' \
  verify_prepared_runtime_readiness >/dev/null 2>&1; then
  fail "missing controller rollout was accepted"
fi
if PATH="${FAKE_BIN}:${PATH}" FAKE_KUBECTL_FAILURE_MATCH='backend-server-wildcard-tls' \
  verify_prepared_runtime_readiness >/dev/null 2>&1; then
  fail "missing wildcard TLS Secret was accepted"
fi
if PATH="${FAKE_BIN}:${PATH}" FAKE_MISSING_PORTS=true \
  verify_prepared_runtime_readiness >/dev/null 2>&1; then
  fail "missing Gateway Service port was accepted"
fi

# shellcheck source=/dev/null
source "${DECISION_FUNCTION_FILE}"
SKIP_PREPARE=false
FORCE_PREPARE=false
STATE_NAMESPACE=edgion-system
STATE_CONFIGMAP_NAME=prepare-state
PREPARE_STATE_OWNER=edgion-k8s-integration-v1
TEST_SERVER_REPLICAS=3
BACKEND_TEST_NAMESPACE=edgion-backend
compute_prepare_conf_hash() { printf 'current-hash\n'; }
get_state_data() {
  case "$1" in
    prepare_ok) printf 'true\n' ;;
    state_owner) printf '%s\n' "${FAKE_STATE_OWNER:-${PREPARE_STATE_OWNER}}" ;;
    conf_hash) printf 'current-hash\n' ;;
    test_server_replicas) printf '3\n' ;;
    backend_test_namespace) printf 'edgion-backend\n' ;;
  esac
}
prepare_images_cacheable() { [[ "${FAKE_IMAGES_CACHEABLE:-true}" == "true" ]]; }
verify_prepared_runtime_readiness() { [[ "${FAKE_RUNTIME_READY:-true}" == "true" ]]; }

if PATH="${FAKE_BIN}:${PATH}" should_run_prepare; then
  fail "healthy immutable cached runtime selected prepare"
fi
REPREPARE_EXISTING=false
PATH="${FAKE_BIN}:${PATH}" FAKE_RUNTIME_READY=false should_run_prepare \
  || fail "runtime drift did not select automatic prepare"
[[ "${REPREPARE_EXISTING}" == "true" ]] \
  || fail "runtime drift did not mark the existing cluster for in-place prepare"
PATH="${FAKE_BIN}:${PATH}" FAKE_IMAGES_CACHEABLE=false should_run_prepare \
  || fail "mutable image identities did not select automatic prepare"
REPREPARE_EXISTING=false
PATH="${FAKE_BIN}:${PATH}" FAKE_STATE_OWNER=foreign should_run_prepare \
  || fail "untrusted state did not select prepare"
[[ "${REPREPARE_EXISTING}" == "false" ]] \
  || fail "untrusted state authorized an in-place prepare"
grep -Fq '"${REPREPARE_EXISTING}" == "false"' "${MAIN_RUNNER}" \
  || fail "in-place prepare still requires a clean cluster boundary"
invalidate_line="$(grep -nF 'invalidate_prepare_state_marker' "${MAIN_RUNNER}" | tail -n 1 | cut -d: -f1)"
namespace_apply_line="$(grep -nF 'kubectl apply --server-side --force-conflicts -f "${K8S_DEPLOY_ROOT}/namespace.yaml"' "${MAIN_RUNNER}" | tail -n 1 | cut -d: -f1)"
[[ -n "${invalidate_line}" && -n "${namespace_apply_line}" && "${invalidate_line}" -lt "${namespace_apply_line}" ]] \
  || fail "in-place prepare does not invalidate the old success marker before cluster mutation"

echo "OK: prepare state readiness and automatic recovery are strict"
