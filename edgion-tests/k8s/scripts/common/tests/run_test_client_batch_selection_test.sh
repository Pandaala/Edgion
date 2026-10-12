#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMMON_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEST_TMP_DIR="$(mktemp -d /tmp/edgion-test-client-batch-selection.XXXXXX)"
FAKE_BIN_DIR="${TEST_TMP_DIR}/bin"
INNER_SCRIPT="${TEST_TMP_DIR}/inner.sh"
SUITES_FILE="${TEST_TMP_DIR}/suites.txt"

cleanup() {
  rm -rf "${TEST_TMP_DIR}"
}
trap cleanup EXIT

PEER_TEST_ROOT="${TEST_TMP_DIR}"
PEER_CONF_ROOT="${SCRIPT_DIR}/../../../conf"

export FAKE_PEER_STORE="${PEER_TEST_ROOT}/peer-store"
mkdir -p "${FAKE_PEER_STORE}"
python3 - "${SCRIPT_DIR}/../render_test_client_peer_fixtures.py" "${SCRIPT_DIR}/../../../conf" "${FAKE_PEER_STORE}" "${PEER_CONF_ROOT}" <<'PEERPY'
import importlib.util
import json
from pathlib import Path
import shutil
import sys
spec = importlib.util.spec_from_file_location("renderer", sys.argv[1])
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
root, store, staged = map(Path, sys.argv[2:])
for row in r.CATALOG:
    if staged != root:
        path = staged / "EdgionPlugins" / row[0]
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(root / "EdgionPlugins" / row[0], path)
for doc in r.render(root, "peer-uid", "10.244.0.7"):
    if doc["kind"] == "EdgionConfigData":
        doc["spec"].setdefault("enable", True)
        doc["spec"].setdefault("visibility", "Namespace")
    (store / (doc["metadata"]["name"] + ".json")).write_text(json.dumps(doc))
identity = {"metadata": {"annotations": {"edgion.io/test-client-pod-uid": "peer-uid", "edgion.io/test-client-pod-ip": "10.244.0.7"}}, "spec": {"data": {"config": {"items": [{"name": "test-client", "cidrs": ["10.244.0.7/32"]}]}}}}
(store / "k8s-test-client-peer.json").write_text(json.dumps(identity))
PEERPY

mkdir -p "${FAKE_BIN_DIR}"
cat > "${SUITES_FILE}" <<'EOF'
EdgionPlugins|BodyPlugins
EdgionPlugins|Guardrail
EOF

cat > "${FAKE_BIN_DIR}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == get && "${2:-}" == pods && "$*" == *"--output json"* ]]; then
  echo '{"items":[{"metadata":{"name":"fake-pod","uid":"peer-uid"},"status":{"phase":"Running","podIP":"10.244.0.7","conditions":[{"type":"Ready","status":"True"}]}}]}'
  exit 0
fi
if [[ "${1:-}" == get && "${2:-}" == pod ]]; then
  printf '{"metadata":{"uid":"%s"},"status":{"podIP":"10.244.0.7"}}\n' "${FAKE_SELECTED_UID:-peer-uid}"
  exit 0
fi
if [[ "${1:-}" == get && ( "${2:-}" == EdgionConfigData || "${2:-}" == EdgionPlugins || ( "${2:-}" == edgionconfigdata && "${3:-}" == k8s-test-client-peer ) ) ]]; then
  cat "${FAKE_PEER_STORE}/${3}.json"
  exit 0
fi

if [ "${1:-}" = get ] && [ "${2:-}" = pods ]; then
  printf 'test-client-pod <none>\n'
  exit 0
fi
if [ "${1:-}" = get ] && [ "${2:-}" = --raw ]; then
  case "$*" in
    *service-name%3Dedgion-test-server*)
      cat <<'JSON'
{"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","items":[{"metadata":{"name":"test-server"},"addressType":"IPv4","ports":[{"name":"http","protocol":"TCP","port":30001}],"endpoints":[{"addresses":["10.0.0.1"],"conditions":{"ready":true}},{"addresses":["10.0.0.2"],"conditions":{"ready":true}}]}]}
JSON
      ;;
    *service-name%3Dedgion-gateway*)
      cat <<'JSON'
{"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","items":[{"metadata":{"name":"gateway"},"addressType":"IPv4","ports":[{"name":"metrics-12090","protocol":"TCP","port":12090}],"endpoints":[{"addresses":["10.0.0.9"],"conditions":{"ready":true}}]}]}
JSON
      ;;
    *)
      echo "unexpected EndpointSlice query: $*" >&2
      exit 9
      ;;
  esac
  exit 0
fi
if [ "${1:-}" = exec ] && [ "${2:-}" = -i ]; then
  cat > "${FAKE_INNER_SCRIPT}"
  if [ "${FAKE_EXECUTE_INNER:-0}" = 1 ]; then
    sed 's@^cd /usr/local/edgion@cd "$FAKE_WORK_DIR"@' "${FAKE_INNER_SCRIPT}" > "${FAKE_INNER_SCRIPT}.run"
    sh "${FAKE_INNER_SCRIPT}.run"
    exit $?
  fi
  printf '@@BATCH_SUMMARY total=2 failed=0\n'
  exit 0
fi
if [ "${1:-}" = exec ]; then
  printf '%s\n' "$*" > "${FAKE_SINGLE_ARGS}"
  exit 0
fi

echo "unexpected kubectl arguments: $*" >&2
exit 9
EOF
chmod 755 "${FAKE_BIN_DIR}/kubectl"

export PATH="${FAKE_BIN_DIR}:${PATH}"
export FAKE_INNER_SCRIPT="${INNER_SCRIPT}"
export EDGION_TEST_GATEWAY_CAPABILITIES='tls=boringssl'
export EDGION_TEST_ADMIN_TOKEN='controller-token'
export EDGION_GATEWAY_ADMIN_TOKEN='gateway-token'

output="$("${COMMON_DIR}/run_test_client_batch.sh" "${SUITES_FILE}")"
case "$output" in
  *'Test server endpoints: 10.0.0.1,10.0.0.2'*) ;;
  *)
    echo "batch runner did not inventory every test-server endpoint" >&2
    echo "$output" >&2
    exit 1
    ;;
esac
grep -Fq 'export EDGION_TEST_BACKEND_ENDPOINT_IPS="10.0.0.1,10.0.0.2"' "${INNER_SCRIPT}"
grep -Fq -- "-i 'BodyPlugins'" "${INNER_SCRIPT}"
grep -Fq -- "-i 'Guardrail'" "${INNER_SCRIPT}"

printf ' Gateway | TLS/BackendTLS \n' > "${SUITES_FILE}"
"${COMMON_DIR}/run_test_client_batch.sh" "${SUITES_FILE}" > /dev/null
grep -Fq 'export EDGION_TEST_BACKEND_ENDPOINT_IPS="10.0.0.1,10.0.0.2"' "${INNER_SCRIPT}"
grep -Fq -- "-i 'TLS/BackendTLS'" "${INNER_SCRIPT}"

# Execute the actual generated Pod script with a fake canonical client.
cat > "${FAKE_BIN_DIR}/test_client" <<'EOF'
#!/bin/sh
resource= item=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -r) resource="$2"; shift ;;
    -i) item="$2"; shift ;;
  esac
  shift
done
if [ "$resource" != TLSRoute ]; then
  echo 'DEFERRED: performed control keeps its existing deferred coverage'
  exit 0
fi
if [ "${FAKE_SKIP_MODE:-canonical}" = canonical ] && [ "$EDGION_INTEGRATION_EXPLICIT_TARGET" = 1 ]; then
  echo 'Error: explicitly requested unsupported TLSRouteModeTerminate' >&2
  exit 1
fi
reason="SKIPPED: suite \`TLSRoute/$item\` requires TLSRouteModeTerminate, which is not currently supported"
case "${FAKE_SKIP_MODE:-canonical}" in
  canonical) echo "$reason" ;;
  duplicate) echo "$reason"; echo "$reason" ;;
  wrong) echo 'SKIPPED: suite `TLSRoute/Other` requires OtherCapability' ;;
  nonzero) echo "$reason"; exit 7 ;;
esac
EOF
chmod +x "${FAKE_BIN_DIR}/test_client"
export FAKE_EXECUTE_INNER=1 FAKE_WORK_DIR="${TEST_TMP_DIR}"
export TEST_CLIENT_BIN_PATH="${FAKE_BIN_DIR}/test_client"
export EDGION_INTEGRATION_EXPLICIT_TARGET=0
source "${COMMON_DIR}/validate_batch_results.sh"
for item in Basic ProxyProtocol StreamPlugins MultiSNI BothAbsentParentRef GeoRestriction; do
  printf 'HTTPRoute|Basic\nTLSRoute|%s\n' "$item" > "${SUITES_FILE}"
  "${COMMON_DIR}/run_test_client_batch.sh" "${SUITES_FILE}" > "${TEST_TMP_DIR}/output"
  edgion_validate_k8s_batch_results "${SUITES_FILE}" "${TEST_TMP_DIR}/output" 0 allow-tls-terminate-skip >/dev/null
  [ "$EDGION_K8S_BATCH_PASSED" -eq 1 ]
  [ "$EDGION_K8S_BATCH_SKIPPED" -eq 1 ]
  grep -q '^DEFERRED: performed control' "${TEST_TMP_DIR}/output"
done
for mode in duplicate wrong nonzero; do
  export FAKE_SKIP_MODE="$mode"
  if "${COMMON_DIR}/run_test_client_batch.sh" "${SUITES_FILE}" > "${TEST_TMP_DIR}/output"; then
    echo "invalid skip was accepted: $mode" >&2
    exit 1
  fi
done
unset FAKE_SKIP_MODE EDGION_INTEGRATION_EXPLICIT_TARGET
if "${COMMON_DIR}/run_test_client_batch.sh" "${SUITES_FILE}" > "${TEST_TMP_DIR}/output"; then
  echo 'standalone batch must default to explicit target failure' >&2
  exit 1
fi
grep -q '@@SUITE_RESULT TLSRoute|GeoRestriction FAIL 1' "${TEST_TMP_DIR}/output"

# Run the main runner's actual selection/accounting function against the same
# generated Pod script. This also protects the parent-shell counters.
MAIN_RUNNER="${COMMON_DIR}/../run_k8s_integration.sh"
sed -n '/^run_all_selected_suites() {/,/^should_run_prepare() {/p' "$MAIN_RUNNER" \
  | sed '$d' > "${TEST_TMP_DIR}/selection.sh"
source "${TEST_TMP_DIR}/selection.sh"
suite_matches_filter() {
  if [ "$FAKE_SELECTED_SET" = mixed ]; then
    [ "$1|$2" = 'HTTPRoute|Basic' ] || [ "$1|$2" = 'TLSRoute|Basic' ]
  else
    [ "$1|$2" = 'TLSRoute|Basic' ]
  fi
}
suite_test_name() { printf '%s_%s\n' "$1" "$2"; }
is_slow_test() { return 1; }
is_host_side_suite() { return 1; }
CONF_ROOT="${TEST_TMP_DIR}/conf"
mkdir -p "$CONF_ROOT/HTTPRoute/Basic" "$CONF_ROOT/TLSRoute/Basic"
K8S_RUN_DIR="${TEST_TMP_DIR}/run"
RUN_CLIENT_BATCH_SCRIPT="${COMMON_DIR}/run_test_client_batch.sh"
CURRENT_ROUND=round-1 FULL_TEST=true START_FROM=""
FILTERED_MODE=true ONLY_RESOURCE=TLSRoute ONLY_ITEM=""
export EDGION_INTEGRATION_EXPLICIT_TARGET=0
for FAKE_SELECTED_SET in mixed skipped; do
  SELECTED_COUNT=0 EXECUTED_COUNT=0 SKIPPED_COUNT=0 FAIL_COUNT=0 MISSING_COUNT=0
  HARNESS_FAILED=false FAILED_SUITES=() MISSING_SUITES=()
  run_all_selected_suites true > "${TEST_TMP_DIR}/selection-output"
  [ "$SKIPPED_COUNT" -eq 1 ]
  [ "$FAIL_COUNT" -eq 0 ]
  if [ "$FAKE_SELECTED_SET" = mixed ]; then
    [ "$SELECTED_COUNT" -eq 2 ] && [ "$EXECUTED_COUNT" -eq 1 ]
  else
    [ "$SELECTED_COUNT" -eq 1 ] && [ "$EXECUTED_COUNT" -eq 0 ]
  fi
  # Use the actual pre-final-gate empty-run/accounting check.
  sed -n '/^if \[\[ "${EXECUTED_COUNT}" -eq 0 \]\]; then/,/^if \[\[ "${FILTERED_MODE}" == "false" \]\]; then/p' "$MAIN_RUNNER" \
    | sed '$d' > "${TEST_TMP_DIR}/accounting.sh"
  [ -s "${TEST_TMP_DIR}/accounting.sh" ]
  source "${TEST_TMP_DIR}/accounting.sh" > /dev/null
  if [ "$FAKE_SELECTED_SET" = mixed ]; then
    [ "$HARNESS_FAILED" = false ]
  else
    [ "$HARNESS_FAILED" = true ]
  fi
done

# The main runner distinguishes resource-wide scans from explicit suite targets.
sed -n '/^if \[\[ -n "${ONLY_RESOURCE}" \&\& -n "${ONLY_ITEM}" \]\]; then/,/^fi$/p' "$MAIN_RUNNER" \
  > "${TEST_TMP_DIR}/explicit-target.sh"
[ -s "${TEST_TMP_DIR}/explicit-target.sh" ]
ONLY_RESOURCE=TLSRoute ONLY_ITEM=""
source "${TEST_TMP_DIR}/explicit-target.sh"
[ "$EDGION_INTEGRATION_EXPLICIT_TARGET" = 0 ]
ONLY_ITEM=Basic
source "${TEST_TMP_DIR}/explicit-target.sh"
[ "$EDGION_INTEGRATION_EXPLICIT_TARGET" = 1 ]

# The individual launcher preserves explicit mode by default and propagates
# the main runner's resource-wide mode without changing client arguments.
export FAKE_SINGLE_ARGS="${TEST_TMP_DIR}/single-args"
unset EDGION_INTEGRATION_EXPLICIT_TARGET
"${COMMON_DIR}/run_test_client.sh" -r TLSRoute -i Basic > /dev/null
grep -Fq 'EDGION_INTEGRATION_EXPLICIT_TARGET=1' "$FAKE_SINGLE_ARGS"
export EDGION_INTEGRATION_EXPLICIT_TARGET=0
"${COMMON_DIR}/run_test_client.sh" -r TLSRoute -i Basic > /dev/null
grep -Fq 'EDGION_INTEGRATION_EXPLICIT_TARGET=0' "$FAKE_SINGLE_ARGS"

export FAKE_SELECTED_UID=stale-selected-pod
if "${COMMON_DIR}/run_test_client_batch.sh" "${SUITES_FILE}" > "${TEST_TMP_DIR}/peer-error" 2>&1; then
  echo "batch accepted a selected Pod outside prepared provenance" >&2
  exit 1
fi
grep -q 'Selected Pod does not match prepared Ready peer identity' "${TEST_TMP_DIR}/peer-error"
if "${COMMON_DIR}/run_test_client.sh" -r HTTPRoute -i Basic > "${TEST_TMP_DIR}/peer-error" 2>&1; then
  echo "individual launcher accepted stale selected Pod" >&2
  exit 1
fi
grep -q 'Selected Pod does not match prepared Ready peer identity' "${TEST_TMP_DIR}/peer-error"
unset FAKE_SELECTED_UID

echo "run_test_client_batch_selection_test.sh: PASS"
