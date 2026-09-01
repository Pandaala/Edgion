#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_RUNNER="${SCRIPT_DIR}/../../run_k8s_integration.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-kubernetes-preflight-test.XXXXXX)"
FUNCTION_FILE="${TEMP_DIR}/functions.sh"
FAKE_BIN="${TEMP_DIR}/bin"

cleanup() {
  rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT INT TERM

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

awk '
  /^preflight_kubernetes_cluster\(\) \{/ { emit = 1 }
  /^get_cluster_arches\(\) \{/ { emit = 0 }
  emit { print }
' "${MAIN_RUNNER}" > "${FUNCTION_FILE}"
grep -Fq 'preflight_kubernetes_cluster()' "${FUNCTION_FILE}" \
  || fail "could not extract Kubernetes preflight function"

mkdir -p "${FAKE_BIN}"

run_preflight() {
  local output_file="$1"
  shift
  env PATH="${FAKE_BIN}:${PATH}" "$@" bash -c \
    'source "$1"; preflight_kubernetes_cluster' _ "${FUNCTION_FILE}" \
    > "${output_file}" 2>&1
}

cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$*" == "config current-context" ]]; then
  if [[ "${FAKE_CONTEXT_MODE:-ready}" == "missing" ]]; then
    exit 1
  fi
  echo "standard-k8s"
  exit 0
fi

if [[ "$*" == "--request-timeout=10s version --output=json" ]]; then
  if [[ "${FAKE_API_MODE:-ready}" == "unreachable" ]]; then
    echo "connection refused" >&2
    exit 1
  fi
  echo '{"serverVersion":{"gitVersion":"v1.33.0"}}'
  exit 0
fi

if [[ "$*" == "--request-timeout=10s get nodes -o json" ]]; then
  case "${FAKE_NODE_MODE:-ready}" in
    empty)
      echo '{"apiVersion":"v1","kind":"NodeList","items":[]}'
      ;;
    not-ready)
      echo '{"apiVersion":"v1","kind":"NodeList","items":[{"status":{"conditions":[{"type":"Ready","status":"False"}],"nodeInfo":{"architecture":"amd64"}}}]}'
      ;;
    mixed)
      echo '{"apiVersion":"v1","kind":"NodeList","items":[{"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"amd64"}}},{"status":{"conditions":[{"type":"Ready","status":"False"}],"nodeInfo":{"architecture":"amd64"}}}]}'
      ;;
    *)
      echo '{"apiVersion":"v1","kind":"NodeList","items":[{"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"amd64"}}}]}'
      ;;
  esac
  exit 0
fi

echo "unexpected kubectl invocation: $*" >&2
exit 1
EOF
chmod +x "${FAKE_BIN}/kubectl"

SUCCESS_OUTPUT="${TEMP_DIR}/success.log"
run_preflight "${SUCCESS_OUTPUT}" FAKE_NODE_MODE=mixed \
  || fail "a reachable standard Kubernetes cluster was rejected"
grep -Fq 'context=standard-k8s ready_nodes=1/2 architectures=amd64' "${SUCCESS_OUTPUT}" \
  || fail "success summary omitted context, readiness, or architecture"
grep -Fq 'also has 1 non-Ready Node(s)' "${SUCCESS_OUTPUT}" \
  || fail "non-Ready Node warning was not emitted"

for case_name in missing-context wrong-context unreachable-api empty-nodes no-ready-nodes; do
  output_file="${TEMP_DIR}/${case_name}.log"
  case "${case_name}" in
    missing-context)
      args=(FAKE_CONTEXT_MODE=missing)
      expected='has no current context'
      ;;
    wrong-context)
      args=(EDGION_K8S_EXPECTED_CONTEXT=production)
      expected='does not match EDGION_K8S_EXPECTED_CONTEXT=production'
      ;;
    unreachable-api)
      args=(FAKE_API_MODE=unreachable)
      expected='API server is not reachable'
      ;;
    empty-nodes)
      args=(FAKE_NODE_MODE=empty)
      expected='has no Nodes'
      ;;
    no-ready-nodes)
      args=(FAKE_NODE_MODE=not-ready)
      expected='has no Ready Nodes'
      ;;
  esac
  if run_preflight "${output_file}" "${args[@]}"; then
    fail "${case_name} unexpectedly passed"
  fi
  grep -Fq "${expected}" "${output_file}" \
    || fail "${case_name} did not report the expected reason"
done

echo "OK: Kubernetes preflight regression tests passed"
