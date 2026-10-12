#!/usr/bin/env bash

set -euo pipefail

MODE="${1:-}"
POD_NAMESPACE="${TEST_CLIENT_NAMESPACE:-edgion-test}"
POD_SELECTOR="${TEST_CLIENT_SELECTOR:-app=edgion-test-client}"
IDENTITY_NAME="${TEST_CLIENT_PEER_IDENTITY_NAME:-k8s-test-client-peer}"
TARGET_NAMESPACES="${TEST_CLIENT_PEER_IDENTITY_NAMESPACES:-edgion-test edgion-default}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF_ROOT="${CONF_ROOT:-${SCRIPT_DIR}/../../conf}"
RENDERER="${SCRIPT_DIR}/render_test_client_peer_fixtures.py"

usage() {
  cat <<'EOF'
Usage: test_client_peer_identity.sh publish|assert|current|prepare|assert-prepared|environment

Publish or verify the canonical EdgionConfigData IpList that represents the
single Ready K8s test-client Pod. The identity is fail-closed: zero, multiple,
terminating, non-Ready, or invalid Pod addresses are rejected.

Modes:
  publish  Server-side apply the identity in every target namespace.
  assert   Verify every published identity still matches the current Pod.
  current  Print the current Pod UID and IP separated by one tab.
  prepare  Observe the physical peer, then apply the nine guarded CIDR fields.
  assert-prepared  Verify identity and all admitted peer fixture contracts.
  environment  Verify preparation and print UID, IP, hit and miss CIDRs.
EOF
}

error() {
  echo "ERROR: $*" >&2
}

for command_name in kubectl jq python3; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    error "${command_name} not found in PATH"
    exit 2
  fi
done

case "${MODE}" in
  publish|assert|current|prepare|assert-prepared|environment) ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

pod_json="$(
  kubectl get pods --request-timeout=5s --namespace "${POD_NAMESPACE}" --selector "${POD_SELECTOR}" --output json
)"
ready_pods="$(
  printf '%s\n' "${pod_json}" | jq -c '[
    .items[]?
    | select(.metadata.deletionTimestamp == null)
    | select(.status.phase == "Running")
    | select(any(.status.conditions[]?; .type == "Ready" and .status == "True"))
  ]'
)"
ready_count="$(printf '%s\n' "${ready_pods}" | jq -r 'length')"
if [ "${ready_count}" -ne 1 ]; then
  error "expected exactly one Running, Ready, non-terminating test-client Pod; found ${ready_count}"
  exit 1
fi

pod_uid="$(printf '%s\n' "${ready_pods}" | jq -r '.[0].metadata.uid // empty')"
pod_ip="$(printf '%s\n' "${ready_pods}" | jq -r '.[0].status.podIP // empty')"
if [ -z "${pod_uid}" ] || [ -z "${pod_ip}" ]; then
  error "the Ready test-client Pod is missing metadata.uid or status.podIP"
  exit 1
fi

pod_cidr="$(python3 - "${pod_ip}" <<'PY'
import ipaddress
import sys

try:
    address = ipaddress.ip_address(sys.argv[1])
except ValueError as error:
    raise SystemExit(f"invalid test-client Pod IP: {error}")

prefix = 32 if address.version == 4 else 128
print(f"{address}/{prefix}")
PY
)"

if [ "${MODE}" = "current" ]; then
  printf '%s\t%s\n' "${pod_uid}" "${pod_ip}"
  exit 0
fi

render_identity() {
  local namespace="$1"
  cat <<EOF
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: ${IDENTITY_NAME}
  namespace: ${namespace}
  annotations:
    edgion.io/test-client-pod-uid: "${pod_uid}"
    edgion.io/test-client-pod-ip: "${pod_ip}"
spec:
  data:
    type: IpList
    config:
      items:
        - name: test-client
          cidrs:
            - "${pod_cidr}"
EOF
}

assert_namespace_identity() {
  local namespace="$1"
  local identity_json actual_uid actual_ip actual_cidr
  identity_json="$(
    kubectl get edgionconfigdata "${IDENTITY_NAME}" --request-timeout=5s --namespace "${namespace}" --output json
  )" || {
    error "missing EdgionConfigData ${namespace}/${IDENTITY_NAME}"
    return 1
  }
  actual_uid="$(
    printf '%s\n' "${identity_json}" | jq -r '.metadata.annotations["edgion.io/test-client-pod-uid"] // empty'
  )"
  actual_ip="$(
    printf '%s\n' "${identity_json}" | jq -r '.metadata.annotations["edgion.io/test-client-pod-ip"] // empty'
  )"
  actual_cidr="$(
    printf '%s\n' "${identity_json}" | jq -r \
      '.spec.data.config.items[]? | select(.name == "test-client") | .cidrs[]?' \
      | paste -sd ',' -
  )"
  if [ "${actual_uid}" != "${pod_uid}" ] \
    || [ "${actual_ip}" != "${pod_ip}" ] \
    || [ "${actual_cidr}" != "${pod_cidr}" ]; then
    error "stale test-client peer identity in ${namespace}/${IDENTITY_NAME}"
    error "expected uid=${pod_uid} ip=${pod_ip} cidr=${pod_cidr}"
    error "actual   uid=${actual_uid:-<empty>} ip=${actual_ip:-<empty>} cidr=${actual_cidr:-<empty>}"
    return 1
  fi
}

for namespace in ${TARGET_NAMESPACES}; do
  if [ "${MODE}" = "publish" ]; then
    render_identity "${namespace}" | kubectl apply \
      --server-side --force-conflicts --field-manager=edgion-k8s-test --filename -
  fi
  assert_namespace_identity "${namespace}"
done

if [[ "${MODE}" == "prepare" ]]; then
  pod_name="$(printf '%s\n' "${ready_pods}" | jq -r '.[0].metadata.name // empty')"
  [[ -n "${pod_name}" ]] || { error "missing Ready Pod name"; exit 1; }
  [[ -n "${EDGION_GATEWAY_ADMIN_TOKEN:-}" ]] || { error "missing Gateway admin token for observed-peer verification"; exit 1; }
  # Readiness plus observation share one wall deadline. Each API connection and
  # curl operation is also bounded; cleanup removes only this exact trace.
  probe_deadline=$(( $(date +%s) + 40 ))
  trace_id="$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
  # This marked request must use the physical Pod connection, without XFF.
  admin_url="${EDGION_TEST_GATEWAY_ADMIN_URL:-http://edgion-gateway.edgion-system.svc.cluster.local:12001}"
  log_url="${admin_url}/api/v1/testing/access-log/${trace_id}"
  cleanup_probe() {
    kubectl --request-timeout=5s exec --namespace "${POD_NAMESPACE}" "${pod_name}" -c test-client -- \
      curl --silent --max-time 2 --noproxy '*' -X DELETE \
      -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN:-}" "${log_url}" >/dev/null 2>&1 || true
  }
  trap cleanup_probe EXIT
  kubectl --request-timeout=5s exec -i --namespace "${POD_NAMESPACE}" "${pod_name}" -c test-client -- \
    env PEER_TRACE_ID="${trace_id}" sh -s <<'PROBE'
set -eu
deadline=$(( $(date +%s) + 20 ))
while ! curl --silent --fail --max-time 2 --noproxy '*' \
  -H 'Host: test.example.com' -H 'access_log: test_store' \
  -H "traceparent: 00-${PEER_TRACE_ID}-0123456789abcdef-00" \
  http://edgion-gateway.edgion-system.svc.cluster.local:31000/echo >/dev/null; do
  [ "$(date +%s)" -lt "$deadline" ] || exit 1
  sleep 1
done
PROBE
  while :; do
    payload="$(kubectl --request-timeout=5s exec --namespace "${POD_NAMESPACE}" "${pod_name}" -c test-client -- \
      curl --silent --fail --max-time 2 --noproxy '*' \
      -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN:-}" "${log_url}")" || payload='{}'
    if printf '%s' "${payload}" | python3 -c 'import json, sys; value=json.load(sys.stdin); info=(value.get("data") or {}).get("request_info") or {}; sys.exit(0 if value.get("success") and info.get("trace_id")==sys.argv[1] and info.get("direct_peer_ip")==sys.argv[2] and info.get("remote_ip")==sys.argv[2] and info.get("host")=="test.example.com" and info.get("path")=="/echo" and info.get("status")==200 else 1)' "${trace_id}" "${pod_ip}"; then
      break
    fi
    [[ "$(date +%s)" -lt "${probe_deadline}" ]] || { error "observed Gateway peer did not match Ready Pod identity"; exit 1; }
    sleep 1
  done
  cleanup_probe
  trap - EXIT
  current_identity="$("${BASH_SOURCE[0]}" current)"
  [[ "${current_identity}" == "${pod_uid}"$'\t'"${pod_ip}" ]] || { error "Pod changed during peer observation"; exit 1; }
  rendered="$(mktemp)"
  trap 'rm -f "${rendered}"' EXIT
  python3 "${RENDERER}" render --conf-root "${CONF_ROOT}" --uid "${pod_uid}" --ip "${pod_ip}" >"${rendered}"
  kubectl apply --server-side --force-conflicts --field-manager=edgion-k8s-test --filename "${rendered}"
  current_identity="$("${BASH_SOURCE[0]}" current)"
  [[ "${current_identity}" == "${pod_uid}"$'\t'"${pod_ip}" ]] || { error "Pod changed while applying peer fixtures"; exit 1; }
fi

if [[ "${MODE}" == "prepare" || "${MODE}" == "assert-prepared" || "${MODE}" == "environment" ]]; then
  renderer_mode=assert
  [[ "${MODE}" != "environment" ]] || renderer_mode=environment
  python3 "${RENDERER}" "${renderer_mode}" --conf-root "${CONF_ROOT}" --uid "${pod_uid}" --ip "${pod_ip}"
  [[ "${MODE}" != "environment" ]] || exit 0
fi

if [ "${MODE}" = "publish" ]; then
  echo "Published test-client peer identity: uid=${pod_uid} ip=${pod_ip} cidr=${pod_cidr}"
else
  echo "Test-client peer identity is current: uid=${pod_uid} ip=${pod_ip} cidr=${pod_cidr}"
fi
