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
  if [[ "${FAKE_NODE_WARNING:-false}" == "true" ]]; then
    echo 'Warning: node listing warning' >&2
  fi
  case "${FAKE_NODE_MODE:-ready}" in
    list)
      echo '{"apiVersion":"v1","kind":"List","items":[{"apiVersion":"v1","kind":"Node","status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"arm64"}}}]}'
      ;;
    non-node)
      echo '{"apiVersion":"v1","kind":"List","items":[{"apiVersion":"v1","kind":"Pod","status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"arm64"}}}]}'
      ;;
    malformed)
      echo '{invalid json'
      ;;
    invalid-items)
      echo '{"apiVersion":"v1","kind":"List","items":{}}'
      ;;
    failed)
      echo 'node listing denied' >&2
      exit 1
      ;;
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

for node_mode in list ready; do
  for warning in false true; do
    output_file="${TEMP_DIR}/${node_mode}-warning-${warning}.log"
    run_preflight "${output_file}" FAKE_NODE_MODE="${node_mode}" FAKE_NODE_WARNING="${warning}" \
      || fail "a valid ${node_mode} collection with warning=${warning} was rejected"
    grep -Fq 'ready_nodes=1/1' "${output_file}" \
      || fail "${node_mode} collection omitted readiness"
    if [[ "${warning}" == "true" ]]; then
      grep -Fq 'Warning: node listing warning' "${output_file}" \
        || fail "${node_mode} collection lost the stderr warning"
    fi
  done
done

for case_name in missing-context wrong-context unreachable-api empty-nodes no-ready-nodes non-node malformed invalid-items failed-list; do
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
    non-node|malformed|invalid-items)
      args=(FAKE_NODE_MODE="${case_name}")
      expected='invalid Node collection'
      ;;
    failed-list)
      args=(FAKE_NODE_MODE=failed)
      expected='cannot list Nodes'
      ;;
  esac
  if run_preflight "${output_file}" "${args[@]}"; then
    fail "${case_name} unexpectedly passed"
  fi
  grep -Fq "${expected}" "${output_file}" \
    || fail "${case_name} did not report the expected reason"
  if [[ "${case_name}" == "failed-list" ]]; then
    grep -Fq 'node listing denied' "${output_file}" \
      || fail "failed node listing lost the stderr diagnostic"
  fi
done

echo "OK: Kubernetes preflight regression tests passed"
