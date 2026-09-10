#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_RUNNER="${SCRIPT_DIR}/../../run_k8s_integration.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-k8s-image-override-test.XXXXXX)"
FUNCTION_FILE="${TEMP_DIR}/override-function.sh"
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
  /^override_suite_test_server_images\(\) \{/ { emit = 1 }
  /^restart_secret_dependent_workloads\(\) \{/ { emit = 0 }
  emit { print }
' "${MAIN_RUNNER}" > "${FUNCTION_FILE}"
grep -Fq 'override_suite_test_server_images()' "${FUNCTION_FILE}" \
  || fail "could not extract image override function"

mkdir -p "${FAKE_BIN}"
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${KUBECTL_LOG}"
case "$*" in
  "get deployment lb-multi-test-server"*) exit 1 ;;
  "get deployments --all-namespaces -l edgion.io/test-server-image=true -o json")
    printf '{"items":[{"metadata":{"namespace":"edgion-test","name":"ordinary-dynamic","labels":{}}},{"metadata":{"namespace":"edgion-test","name":"direct-port-unready","labels":{"edgion.io/test-server-rollout-ready":"false"}}}]}\n'
    ;;
  "get pods -n edgion-test -l app=direct-port-unready -o json")
    printf '{"items":[{"metadata":{},"status":{"phase":"Running"},"spec":{"containers":[{"name":"test-server","image":"registry.test/test-server:current"}]}}]}\n'
    ;;
esac
EOF
chmod +x "${FAKE_BIN}/kubectl"

SKIP_DEPLOY=false
TEST_SERVER_IMAGE=registry.test/test-server:current
PULL_POLICY_OVERRIDE=Never
export KUBECTL_LOG

# shellcheck source=/dev/null
source "${FUNCTION_FILE}"
PATH="${FAKE_BIN}:${PATH}" override_suite_test_server_images

grep -Fq 'rollout status deployment/ordinary-dynamic -n edgion-test --timeout=300s' "${KUBECTL_LOG}" \
  || fail "ordinary dynamic Deployment did not wait for rollout readiness"
grep -Fq 'get pods -n edgion-test -l app=direct-port-unready -o json' "${KUBECTL_LOG}" \
  || fail "intentionally unready Deployment did not inspect its Running Pod"
if grep -Fq 'rollout status deployment/direct-port-unready' "${KUBECTL_LOG}"; then
  fail "intentionally unready Deployment waited for rollout readiness"
fi

echo "OK: suite image overrides preserve intentionally unready Pods"
