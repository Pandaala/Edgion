#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_RUNNER="${SCRIPT_DIR}/../../run_k8s_integration.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-local-test-binary-mode.XXXXXX)"
FUNCTION_FILE="${TEMP_DIR}/functions.sh"
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

assert_contains() {
  local file="$1"
  local expected="$2"
  if ! grep -Fq -- "${expected}" "${file}"; then
    echo "ERROR: expected output to contain: ${expected}" >&2
    cat "${file}" >&2
    exit 1
  fi
}

awk '
  /^patch_local_binary_deployment\(\) \{/ { emit = 1 }
  /^label_single_backend_pod\(\) \{/ { emit = 0 }
  emit { print }
  /^kubectl_retry\(\) \{/ { retry = 1 }
  /^copy_local_core_binary_to_pods\(\) \{/ { retry = 0 }
  retry { print }
  /^install_local_test_client_tools\(\) \{/ { install = 1 }
  /^sync_k8s_redis_auth_secret\(\) \{/ { install = 0 }
  install { print }
' "${MAIN_RUNNER}" > "${FUNCTION_FILE}"

for function_name in \
  patch_local_binary_test_deployments \
  rollout_local_binary_test_deployments \
  copy_local_binary_to_pods \
  copy_local_test_server_to_suite_pods \
  wait_local_suite_test_server_deployments \
  start_local_test_server_pods \
  install_local_test_binaries; do
  grep -Fq "${function_name}()" "${FUNCTION_FILE}" \
    || fail "could not extract ${function_name} from the main runner"
done

K8S_ROOT="$(cd "$(dirname "${MAIN_RUNNER}")/.." && pwd)"
python3 - "${K8S_ROOT}/conf" <<'PY'
import pathlib
import sys

import yaml

root = pathlib.Path(sys.argv[1])
fixtures = sorted(root.glob("**/DynamicTest/bootstrap/*.yaml"))
labeled = []
for fixture in fixtures:
    with fixture.open(encoding="utf-8") as stream:
        for document in yaml.safe_load_all(stream):
            if not isinstance(document, dict) or document.get("kind") != "Deployment":
                continue
            if document.get("metadata", {}).get("labels", {}).get("edgion.io/test-server-image") != "true":
                continue
            labeled.append(fixture)
            pod_labels = document.get("spec", {}).get("template", {}).get("metadata", {}).get("labels", {})
            if pod_labels.get("edgion.io/test-server-image") != "true":
                raise SystemExit(f"suite image discovery label is missing from Pod template: {fixture}")
if len(labeled) != 6:
    raise SystemExit(f"expected 6 suite test-server bootstrap Deployments, found {len(labeled)}")
PY

# shellcheck source=/dev/null
source "${FUNCTION_FILE}"

mkdir -p "${FAKE_BIN}"
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

printf 'KUBECTL:%s\n' "$*" >> "${KUBECTL_LOG}"

if [[ "$*" == "get deployments --all-namespaces -l edgion.io/test-server-image=true -o json" ]]; then
  printf '{"items":[{"metadata":{"namespace":"edgion-test","name":"aes-tls-dynamic-tls","labels":{}}},{"metadata":{"namespace":"edgion-test","name":"direct-port-unready","labels":{"edgion.io/test-server-rollout-ready":"false"}}}]}\n'
  exit 0
fi
if [[ "$*" == "get deployments -n edgion-test -l edgion.io/test-server-image=true -o json" ]]; then
  printf '{"items":[{"spec":{"replicas":1}},{"spec":{"replicas":1}}]}\n'
  exit 0
fi
if [[ "$*" == "get deployment lb-multi-test-server -n edgion-default" ]]; then
  exit 1
fi
if [[ "$1" == "get" && "$2" == "deployment" ]]; then
  exit 0
fi
if [[ "$1" == "get" && "$2" == "pods" ]]; then
  if [[ " $* " == *" -o json "* ]]; then
    case "$*" in
      *"-l edgion.io/test-server-image=true"*)
        printf '{"items":[{"metadata":{"name":"suite-one"},"status":{"phase":"Running"},"spec":{"containers":[{"name":"test-server","image":"edgion-test-runtime:local"}]}},{"metadata":{"name":"terminating-suite","deletionTimestamp":"2026-09-09T00:00:00Z"},"status":{"phase":"Running"},"spec":{"containers":[{"name":"test-server","image":"edgion-test-runtime:local"}]}},{"metadata":{"name":"suite-two"},"status":{"phase":"Running"},"spec":{"containers":[{"name":"test-server","image":"edgion-test-runtime:local"}]}}]}\n'
        ;;
      *"-l app=direct-port-unready"*)
        printf '{"items":[{"metadata":{"name":"direct-port-unready"},"status":{"phase":"Running"},"spec":{"containers":[{"name":"test-server","image":"edgion-test-runtime:local"}]}}]}\n'
        ;;
      *)
        echo "unexpected JSON Pod selector: $*" >&2
        exit 1
        ;;
    esac
    exit 0
  fi
  namespace=""
  while [[ "$#" -gt 0 ]]; do
    if [[ "$1" == "-n" ]]; then
      namespace="$2"
      break
    fi
    shift
  done
  printf 'pod-%s\n' "${namespace}"
fi
EOF
chmod +x "${FAKE_BIN}/kubectl"

local_test_binary_mode() {
  return 0
}
require_local_test_binaries() {
  return 0
}
label_single_backend_pod() {
  return 0
}

LOCAL_TEST_BIN_DIR="${TEMP_DIR}/local-bins"
LOCAL_TEST_RUNTIME_IMAGE="edgion-test-runtime:local"
BACKEND_TEST_NAMESPACE="edgion-backend"
export KUBECTL_LOG

: > "${KUBECTL_LOG}"
PATH="${FAKE_BIN}:${PATH}" install_local_test_binaries >/dev/null \
  || fail "LOCAL_TEST_BIN_DIR installation path failed"

for namespace in edgion-deleg-dyn edgion-grpc-deleg-dyn; do
  patch_line="$(grep -F \
    "KUBECTL:patch deployment edgion-test-server -n ${namespace} --type=strategic" \
    "${KUBECTL_LOG}")"
  [[ -n "${patch_line}" ]] || fail "${namespace} deployment was not patched"
  [[ "${patch_line}" == *'"image":"edgion-test-runtime:local"'* ]] \
    || fail "${namespace} deployment did not receive the local runtime image"
  assert_contains "${KUBECTL_LOG}" \
    "KUBECTL:rollout status deployment/edgion-test-server -n ${namespace} --timeout=300s"
  assert_contains "${KUBECTL_LOG}" \
    "KUBECTL:cp -n ${namespace} -c test-server ${LOCAL_TEST_BIN_DIR}/test_server pod-${namespace}:/usr/local/bin/test_server"
  assert_contains "${KUBECTL_LOG}" \
    "KUBECTL:exec -n ${namespace} -c test-server pod-${namespace} -- sh -c"

  start_line="$(grep -F "KUBECTL:exec -n ${namespace} -c test-server pod-${namespace} -- sh -c" "${KUBECTL_LOG}" \
    | grep -F 'nohup /usr/local/bin/test_server')"
  [[ -n "${start_line}" ]] || fail "${namespace} test_server was not manually started"
  [[ "${start_line}" != *"--https-backend-wildcard-port"* ]] \
    || fail "${namespace} unexpectedly received TLS-only wildcard arguments"
done

suite_patch_line="$(grep -F \
  'KUBECTL:patch deployment/aes-tls-dynamic-tls -n edgion-test --type=strategic' \
  "${KUBECTL_LOG}")"
[[ "${suite_patch_line}" == *'while [ ! -x /usr/local/bin/test_server ]'* ]] \
  || fail "suite Deployment does not wait for the injected binary"
[[ "${suite_patch_line}" == *'exec /usr/local/bin/test_server \"$@\"'* ]] \
  || fail "suite Deployment does not preserve fixture arguments"
[[ "${suite_patch_line}" != *'"args"'* ]] \
  || fail "suite Deployment patch overwrote fixture arguments"
if grep -Fq 'KUBECTL:patch deployment/direct-port-unready -n edgion-test --type=json' "${KUBECTL_LOG}"; then
  fail "intentionally unready suite Deployment probe was removed"
fi
assert_contains "${KUBECTL_LOG}" \
  "KUBECTL:cp -n edgion-test -c test-server ${LOCAL_TEST_BIN_DIR}/test_server suite-one:/usr/local/bin/test_server"
if grep -Fq 'terminating-suite:/usr/local/bin/test_server' "${KUBECTL_LOG}"; then
  fail "terminating suite Pod was selected for binary copy"
fi
assert_contains "${KUBECTL_LOG}" \
  'KUBECTL:rollout status deployment/aes-tls-dynamic-tls -n edgion-test --timeout=300s'
assert_contains "${KUBECTL_LOG}" \
  'KUBECTL:get pods -n edgion-test -l app=direct-port-unready -o json'
if grep -Fq 'KUBECTL:rollout status deployment/direct-port-unready' "${KUBECTL_LOG}"; then
  fail "intentionally unready suite Deployment waited for readiness"
fi

tls_start_line="$(grep -F 'KUBECTL:exec -n edgion-test -c test-server pod-edgion-test -- sh -c' "${KUBECTL_LOG}" \
  | grep -F 'nohup /usr/local/bin/test_server')"
[[ -n "${tls_start_line}" ]] || fail "edgion-test test_server was not manually started"
for argument in \
  '--https-backend-wildcard-port=30028' \
  '--wildcard-cert=/etc/edgion/backend-wildcard/tls.crt' \
  '--wildcard-key=/etc/edgion/backend-wildcard/tls.key'; do
  [[ "${tls_start_line}" == *"${argument}"* ]] \
    || fail "manual TLS test_server start is missing ${argument}"
done

echo "OK: LOCAL_TEST_BIN_DIR deployment coverage regression tests passed"
