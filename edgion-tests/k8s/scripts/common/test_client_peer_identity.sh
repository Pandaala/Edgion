#!/usr/bin/env bash

set -euo pipefail

MODE="${1:-}"
POD_NAMESPACE="${TEST_CLIENT_NAMESPACE:-edgion-test}"
POD_SELECTOR="${TEST_CLIENT_SELECTOR:-app=edgion-test-client}"
IDENTITY_NAME="${TEST_CLIENT_PEER_IDENTITY_NAME:-k8s-test-client-peer}"
TARGET_NAMESPACES="${TEST_CLIENT_PEER_IDENTITY_NAMESPACES:-edgion-test edgion-default}"

usage() {
  cat <<'EOF'
Usage: test_client_peer_identity.sh publish|assert|current

Publish or verify the canonical EdgionConfigData IpList that represents the
single Ready K8s test-client Pod. The identity is fail-closed: zero, multiple,
terminating, non-Ready, or invalid Pod addresses are rejected.

Modes:
  publish  Server-side apply the identity in every target namespace.
  assert   Verify every published identity still matches the current Pod.
  current  Print the current Pod UID and IP separated by one tab.
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
  publish|assert|current) ;;
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
  kubectl get pods --namespace "${POD_NAMESPACE}" --selector "${POD_SELECTOR}" --output json
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
    kubectl get edgionconfigdata "${IDENTITY_NAME}" --namespace "${namespace}" --output json
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

if [ "${MODE}" = "publish" ]; then
  echo "Published test-client peer identity: uid=${pod_uid} ip=${pod_ip} cidr=${pod_cidr}"
else
  echo "Test-client peer identity is current: uid=${pod_uid} ip=${pod_ip} cidr=${pod_cidr}"
fi
