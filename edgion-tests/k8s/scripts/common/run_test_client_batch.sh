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

SUITES_FILE="${1:-}"
if [[ -z "${SUITES_FILE}" || ! -f "${SUITES_FILE}" ]]; then
  echo "Usage: $0 <suites-file>"
  exit 1
fi

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH"
  exit 1
fi

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

# --- Resolve DirectEndpoint IP (once) ---
# The gateway admin API (port 12001) is loopback-only and is no longer reachable
# over Service DNS, so kubectl is the primary (and only) resolver here.
DIRECT_ENDPOINT_IP="${EDGION_TEST_DIRECT_ENDPOINT_IP:-}"
if [[ -z "${DIRECT_ENDPOINT_IP}" ]]; then
  # EndpointSlice readiness lags pod scheduling, so retry briefly before giving up.
  for _ in $(seq 1 15); do
    DIRECT_ENDPOINT_IP="$(
      kubectl get endpointslices -n edgion-default \
        -l kubernetes.io/service-name=direct-endpoint-backend -o json 2>/dev/null \
        | jq -r 'first(.items[]?.endpoints[]? | select(.conditions.ready != false) | .addresses[]?) // empty' 2>/dev/null || true
    )"
    if [[ -z "${DIRECT_ENDPOINT_IP}" ]]; then
      DIRECT_ENDPOINT_IP="$(
        kubectl get endpoints -n edgion-default direct-endpoint-backend \
          -o jsonpath='{.subsets[0].addresses[0].ip}' 2>/dev/null || true
      )"
    fi
    [[ -n "${DIRECT_ENDPOINT_IP}" ]] && break
    sleep 1
  done
fi

if [[ -n "${DIRECT_ENDPOINT_IP}" ]]; then
  echo "DirectEndpoint IP: ${DIRECT_ENDPOINT_IP}"
else
  # The direct-endpoint-backend fixture is only deployed for the DirectEndpoint
  # test, so an empty result is expected for every other resource/item. Warn and
  # continue with an empty IP; the DirectEndpoint test's own assertions will fail
  # if the IP is genuinely required but missing.
  echo "WARNING: DirectEndpoint IP not resolved (kubectl endpointslices/endpoints both empty); continuing with empty EDGION_TEST_DIRECT_ENDPOINT_IP"
fi

TEST_SERVER_ENDPOINT_IPS="$(
  kubectl get endpointslices -n edgion-test \
    -l kubernetes.io/service-name=edgion-test-server -o json 2>/dev/null \
    | jq -r '[.items[]?.endpoints[]? | select(.conditions.ready != false) | .addresses[]?] | unique | join(",")' 2>/dev/null || true
)"
if [[ -z "${TEST_SERVER_ENDPOINT_IPS}" || "${TEST_SERVER_ENDPOINT_IPS}" == "null" ]]; then
  TEST_SERVER_ENDPOINT_IPS="$(
    kubectl get endpoints -n edgion-test edgion-test-server -o json 2>/dev/null \
      | jq -r '[.subsets[]?.addresses[]?.ip] | unique | join(",")' 2>/dev/null || true
  )"
fi
echo "Test server endpoints: ${TEST_SERVER_ENDPOINT_IPS:-<none>}"

resolve_ready_service_endpoint_ips() {
  local namespace="$1" service="$2" ips
  ips="$(
    kubectl get endpointslices -n "${namespace}" \
      -l "kubernetes.io/service-name=${service}" -o json 2>/dev/null \
      | jq -r '[.items[]?.endpoints[]? | select(.conditions.ready != false) | .addresses[]?] | unique | join(",")' 2>/dev/null || true
  )"
  if [[ -z "${ips}" || "${ips}" == "null" ]]; then
    ips="$(
      kubectl get endpoints -n "${namespace}" "${service}" -o json 2>/dev/null \
        | jq -r '[.subsets[]?.addresses[]?.ip] | unique | join(",")' 2>/dev/null || true
    )"
  fi
  echo "${ips}"
}

LB_RR_EPS_ENDPOINT_IPS="$(resolve_ready_service_endpoint_ips edgion-default lb-rr-service)"
LB_RR_MULTI_ENDPOINT_IPS="$(resolve_ready_service_endpoint_ips edgion-default lb-rr-multi-service)"
echo "RoundRobin endpoints: eps=${LB_RR_EPS_ENDPOINT_IPS:-<none>} multi=${LB_RR_MULTI_ENDPOINT_IPS:-<none>}"

# --- Resolve Gateway Metrics Endpoints (once) ---
GATEWAY_METRICS_ENDPOINTS="${EDGION_TEST_GATEWAY_METRICS_ENDPOINTS:-}"
if [[ -z "${GATEWAY_METRICS_ENDPOINTS}" ]]; then
  GATEWAY_METRICS_ENDPOINTS="$(
    kubectl get endpointslices -n edgion-system -l kubernetes.io/service-name=edgion-gateway -o json 2>/dev/null \
      | jq -r '[.items[]?.endpoints[]? | select(.conditions.ready != false) | .addresses[]? | "\(.):12090"] | unique | join(",")' 2>/dev/null || true
  )"
fi

if [[ -z "${GATEWAY_METRICS_ENDPOINTS}" || "${GATEWAY_METRICS_ENDPOINTS}" == "null" ]]; then
  GATEWAY_METRICS_ENDPOINTS="$(
    kubectl get endpoints -n edgion-system edgion-gateway -o json 2>/dev/null \
      | jq -r '[.subsets[]?.addresses[]?.ip | "\(.):12090"] | unique | join(",")' 2>/dev/null || true
  )"
fi

if [[ -z "${GATEWAY_METRICS_ENDPOINTS}" || "${GATEWAY_METRICS_ENDPOINTS}" == "null" ]]; then
  GATEWAY_METRICS_ENDPOINTS="edgion-gateway.edgion-system.svc.cluster.local:12090"
fi

METRICS_ENDPOINT_COUNT="$(echo "${GATEWAY_METRICS_ENDPOINTS}" | tr ',' '\n' | awk 'NF{count++} END{print count+0}')"
echo "Gateway metrics endpoints (${METRICS_ENDPOINT_COUNT}): ${GATEWAY_METRICS_ENDPOINTS}"

# --- Build in-pod script from suites file, pipe via stdin ---
# Using heredoc-via-stdin avoids shell quoting/escaping issues with large scripts.
generate_inner_script() {
  cat <<HEADER
#!/bin/sh
export EDGION_TEST_ADMIN_API_URL="${EDGION_CONTROLLER_ADMIN_URL:-http://edgion-controller-leader.edgion-system.svc.cluster.local:12101}"
export EDGION_TEST_ADMIN_TOKEN="${EDGION_TEST_ADMIN_TOKEN:-}"
export EDGION_TOKEN="${EDGION_TOKEN:-${EDGION_TEST_ADMIN_TOKEN}}"
export EDGION_GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-}"
export EDGION_TEST_K8S_MODE="true"
export EDGION_TEST_DIRECT_ENDPOINT_IP="${DIRECT_ENDPOINT_IP}"
export EDGION_TEST_BACKEND_ENDPOINT_IPS="${TEST_SERVER_ENDPOINT_IPS}"
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
