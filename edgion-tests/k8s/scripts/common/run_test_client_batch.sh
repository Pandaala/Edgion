#!/usr/bin/env bash
set -euo pipefail

# Run multiple test suites in a single kubectl exec call.
#
# Usage: run_test_client_batch.sh <suites-file>
#
# suites-file: one suite per line, format: resource|item
#   e.g.  HTTPRoute|Basic
#         EdgionPlugins|JwtAuth
#
# Outputs per-suite results as tagged lines for the caller to parse:
#   @@SUITE_START resource|item
#   ... test output ...
#   @@SUITE_RESULT resource|item PASS|FAIL exit_code
#   @@BATCH_SUMMARY total=N failed=M

NAMESPACE="${NAMESPACE:-edgion-test}"
POD="${POD:-}"
CONTAINER="${CONTAINER:-test-client}"
TARGET_HOST="${TARGET_HOST:-edgion-gateway.edgion-system.svc.cluster.local}"
TEST_CLIENT_BIN_PATH="${TEST_CLIENT_BIN_PATH:-/usr/local/bin/test_client}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENDPOINT_INVENTORY="${SCRIPT_DIR}/endpoint_inventory.sh"
PLUGIN_VALIDATION_ADMISSION="${SCRIPT_DIR}/verify_plugin_validation_admission.sh"

SUITES_FILE="${1:-}"
if [[ -z "${SUITES_FILE}" || ! -f "${SUITES_FILE}" ]]; then
  echo "Usage: $0 <suites-file>"
  exit 1
fi

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH"
  exit 1
fi
if [[ ! -x "${ENDPOINT_INVENTORY}" ]]; then
  echo "EndpointSlice inventory helper not found or not executable: ${ENDPOINT_INVENTORY}"
  exit 1
fi
# shellcheck source=endpoint_inventory.sh
source "${ENDPOINT_INVENTORY}"

# --- Resolve Pod name (once) ---
if [[ -z "${POD}" ]]; then
  POD="$(
    kubectl get pods -n "${NAMESPACE}" -l app=edgion-test-client \
      --field-selector=status.phase=Running \
      --sort-by=.metadata.creationTimestamp \
      -o custom-columns=NAME:.metadata.name,DELETING:.metadata.deletionTimestamp \
      --no-headers 2>/dev/null \
      | awk '$2=="<none>" {print $1}' \
      | tail -n 1
  )"
fi

if [[ -z "${POD}" ]]; then
  echo "Cannot find test-client pod in namespace ${NAMESPACE}"
  exit 1
fi

echo "Using pod: ${POD}"
echo "Target host: ${TARGET_HOST}"

K8S_ADMISSION_VERIFIED="${EDGION_TEST_K8S_ADMISSION_VERIFIED:-}"
if grep -Eq '^[[:space:]]*EdgionPlugins[[:space:]]*\|[[:space:]]*PluginValidationStatus[[:space:]]*$' "${SUITES_FILE}"; then
  "${PLUGIN_VALIDATION_ADMISSION}"
  K8S_ADMISSION_VERIFIED="PluginValidationStatus"
fi

DIRECT_ENDPOINT_IP="${EDGION_TEST_DIRECT_ENDPOINT_IP:-}"
if [[ -z "${DIRECT_ENDPOINT_IP}" ]] \
  && grep -Eq '^EdgionPlugins\|DirectEndpoint[[:space:]]*$' "${SUITES_FILE}"; then
  DIRECT_ENDPOINT_IP="$(
    "${ENDPOINT_INVENTORY}" --namespace edgion-default \
      --service direct-endpoint-backend --port http --timeout-secs 30 \
      | jq -r '.[0].address'
  )"
fi
if [[ -n "${DIRECT_ENDPOINT_IP}" ]]; then
  echo "DirectEndpoint IP: ${DIRECT_ENDPOINT_IP}"
fi

TEST_SERVER_ENDPOINT_IPS="${EDGION_TEST_BACKEND_ENDPOINT_IPS:-}"
if grep -Eq '^[[:space:]]*EdgionBackend[[:space:]]*\|' "${SUITES_FILE}"; then
  TEST_SERVER_EXPECTED_REPLICAS="$(
    endpoint_inventory_strict_deployment_replicas edgion-test edgion-test-server
  )"
  TEST_SERVER_ENDPOINT_IPS="$("${ENDPOINT_INVENTORY}" --namespace edgion-test \
    --service edgion-test-server --port 30001 \
    --expected-count "${TEST_SERVER_EXPECTED_REPLICAS}" --timeout-secs 30 --output ips)"
elif [[ -z "${TEST_SERVER_ENDPOINT_IPS}" ]] \
  && grep -Eq '^EdgionPlugins\|(RequestMirror|CredentialInjector|RequestBodyBuffer|BodyPlugins|Guardrail)[[:space:]]*$' "${SUITES_FILE}"; then
  TEST_SERVER_ENDPOINT_IPS="$("${ENDPOINT_INVENTORY}" --namespace edgion-test \
    --service edgion-test-server --port 30001 --timeout-secs 30 --output ips)"
fi
if [[ -n "${TEST_SERVER_ENDPOINT_IPS}" ]]; then
  echo "Test server endpoints: ${TEST_SERVER_ENDPOINT_IPS}"
fi

LB_RR_EPS_ENDPOINT_IPS="${EDGION_TEST_LB_RR_EPS_ENDPOINT_IPS:-}"
LB_RR_MULTI_ENDPOINT_IPS="${EDGION_TEST_LB_RR_MULTI_ENDPOINT_IPS:-}"
if grep -Eq '^HTTPRoute\|(Backend/LBRoundRobin|Backend)[[:space:]]*$' "${SUITES_FILE}"; then
  LB_RR_EPS_HEALTH_ENDPOINTS="$(endpoint_inventory_wait_for_backend_health \
    edgion-default lb-rr-service 30001 3 "${NAMESPACE}" "${POD}" "${CONTAINER}" 90)"
  LB_RR_MULTI_HEALTH_ENDPOINTS="$(endpoint_inventory_wait_for_backend_health \
    edgion-default lb-rr-multi-service 30001 4 "${NAMESPACE}" "${POD}" "${CONTAINER}" 90)"
  if [[ -z "${LB_RR_EPS_ENDPOINT_IPS}" ]]; then
    LB_RR_EPS_ENDPOINT_IPS="$("${ENDPOINT_INVENTORY}" --namespace edgion-default \
      --service lb-rr-service --port 30001 --expected-count 3 --timeout-secs 30 --output ips)"
  fi
  if [[ -z "${LB_RR_MULTI_ENDPOINT_IPS}" ]]; then
    LB_RR_MULTI_ENDPOINT_IPS="$("${ENDPOINT_INVENTORY}" --namespace edgion-default \
      --service lb-rr-multi-service --port 30001 --expected-count 4 --timeout-secs 30 --output ips)"
  fi
  echo "RoundRobin endpoints: eps=${LB_RR_EPS_ENDPOINT_IPS} multi=${LB_RR_MULTI_ENDPOINT_IPS}"
fi

# --- Resolve Gateway Metrics Endpoints (once) ---
GATEWAY_METRICS_ENDPOINTS="${EDGION_TEST_GATEWAY_METRICS_ENDPOINTS:-}"
if [[ -z "${GATEWAY_METRICS_ENDPOINTS}" ]]; then
  GATEWAY_METRICS_ENDPOINTS="$("${ENDPOINT_INVENTORY}" --namespace edgion-system \
    --service edgion-gateway --port 12090 --expected-count 1 --timeout-secs 30 --output hostports)"
fi

METRICS_ENDPOINT_COUNT="$(echo "${GATEWAY_METRICS_ENDPOINTS}" | tr ',' '\n' | awk 'NF{count++} END{print count+0}')"
echo "Gateway metrics endpoints (${METRICS_ENDPOINT_COUNT}): ${GATEWAY_METRICS_ENDPOINTS}"

GATEWAY_CAPABILITIES="${EDGION_TEST_GATEWAY_CAPABILITIES:-}"
if [[ -z "${GATEWAY_CAPABILITIES}" ]]; then
  GATEWAY_CAPABILITIES="$(
    kubectl exec -n edgion-system deployment/edgion-gateway -c edgion-gateway -- sh -c '
      if [ -x /usr/local/edgion/edgion-gateway ]; then
        exec /usr/local/edgion/edgion-gateway --print-build-capabilities
      fi
      exec /usr/local/bin/edgion-gateway --print-build-capabilities
    '
  )"
fi
if [[ -z "${GATEWAY_CAPABILITIES}" ]]; then
  echo "Gateway capability probe returned an empty result" >&2
  exit 1
fi

# --- Build in-pod script from suites file, pipe via stdin ---
# Using heredoc-via-stdin avoids shell quoting/escaping issues with large scripts.
generate_inner_script() {
  cat <<HEADER
#!/bin/sh
export EDGION_TEST_ADMIN_API_URL="${EDGION_CONTROLLER_ADMIN_URL:-http://edgion-controller-leader.edgion-system.svc.cluster.local:12101}"
export EDGION_TEST_ADMIN_TOKEN="${EDGION_TEST_ADMIN_TOKEN:-}"
export EDGION_TOKEN="${EDGION_TOKEN:-${EDGION_TEST_ADMIN_TOKEN}}"
export EDGION_GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-}"
export EDGION_TEST_GATEWAY_CAPABILITIES="${GATEWAY_CAPABILITIES}"
export EDGION_TEST_K8S_ADMISSION_VERIFIED="${K8S_ADMISSION_VERIFIED}"
export EDGION_TEST_K8S_MODE="true"
export TEST_SERVER_HOST="${TEST_SERVER_HOST:-edgion-test-server.edgion-test.svc.cluster.local}"
export EDGION_TEST_DIRECT_ENDPOINT_IP="${DIRECT_ENDPOINT_IP}"
export EDGION_TEST_BACKEND_ENDPOINT_IPS="${TEST_SERVER_ENDPOINT_IPS}"
export EDGION_TEST_DNS_SERVER_HOST="${EDGION_TEST_DNS_SERVER_HOST:-dns-test-server.edgion-test.svc.cluster.local}"
export EDGION_TEST_MIRROR_SERVER_HOST="${EDGION_TEST_MIRROR_SERVER_HOST:-percent-mirror-test-server.edgion-test.svc.cluster.local}"
export EDGION_TEST_TRANSPORT_RECORDER_HOST="${EDGION_TEST_TRANSPORT_RECORDER_HOST:-transport-recorder.edgion-test.svc.cluster.local}"
export EDGION_TEST_WASM_CALLOUT_ORIGIN="${EDGION_TEST_WASM_CALLOUT_ORIGIN:-http://wasm-auth-server.edgion-test.svc.cluster.local:30040}"
export EDGION_TEST_GUARDRAIL_FIXTURE_HOST="${EDGION_TEST_GUARDRAIL_FIXTURE_HOST:-edgion-test-server.edgion-test.svc.cluster.local}"
export EDGION_TEST_LB_RR_EPS_ENDPOINT_IPS="${LB_RR_EPS_ENDPOINT_IPS}"
export EDGION_TEST_LB_RR_MULTI_ENDPOINT_IPS="${LB_RR_MULTI_ENDPOINT_IPS}"
export EDGION_TEST_GATEWAY_METRICS_ENDPOINTS="${GATEWAY_METRICS_ENDPOINTS}"
TEST_BIN="${TEST_CLIENT_BIN_PATH}"
TARGET="${TARGET_HOST}"
TOTAL=0
FAIL=0
cd /usr/local/edgion || exit 1
HEADER

  while IFS='|' read -r resource item || [[ -n "${resource}" ]]; do
    [[ -z "${resource}" || "${resource}" == "#"* ]] && continue
    resource="$(echo "${resource}" | tr -d '[:space:]')"
    item="$(echo "${item}" | tr -d '[:space:]')"

    cat <<SUITE
echo '@@SUITE_START ${resource}|${item}'
TOTAL=\$((TOTAL + 1))
if "\${TEST_BIN}" -g --target-host "\${TARGET}" -r '${resource}' -i '${item}'; then
  echo '@@SUITE_RESULT ${resource}|${item} PASS 0'
else
  _rc=\$?
  echo "@@SUITE_RESULT ${resource}|${item} FAIL \${_rc}"
  FAIL=\$((FAIL + 1))
fi
SUITE
  done < "${SUITES_FILE}"

  cat <<'FOOTER'
echo "@@BATCH_SUMMARY total=$TOTAL failed=$FAIL"
if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
exit 0
FOOTER
}

# --- Execute all suites in one kubectl exec via stdin ---
generate_inner_script | kubectl exec -i -n "${NAMESPACE}" -c "${CONTAINER}" "${POD}" -- sh
