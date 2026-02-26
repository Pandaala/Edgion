#!/usr/bin/env bash
set -euo pipefail

# Resolve a current backend Pod IP for DirectEndpoint tests from Gateway admin API.
#
# Env overrides:
#   GATEWAY_ADMIN_URL   default: http://edgion-gateway.edgion-system.svc.cluster.local:5900
#   SERVICE_NAMESPACE   default: edgion-default
#   SERVICE_NAME        default: direct-endpoint-backend
#   SERVICE_PORT        default: 30001

GATEWAY_ADMIN_URL="${GATEWAY_ADMIN_URL:-http://edgion-gateway.edgion-system.svc.cluster.local:5900}"
SERVICE_NAMESPACE="${SERVICE_NAMESPACE:-edgion-default}"
SERVICE_NAME="${SERVICE_NAME:-direct-endpoint-backend}"
SERVICE_PORT="${SERVICE_PORT:-30001}"

if ! command -v curl >/dev/null 2>&1; then
  exit 2
fi

if ! command -v jq >/dev/null 2>&1; then
  exit 2
fi

slice_json="$(curl -fsS "${GATEWAY_ADMIN_URL}/configclient/EndpointSlice/list" 2>/dev/null || true)"
if [[ -n "${slice_json}" ]]; then
  ip_from_slice="$(
    echo "${slice_json}" | jq -r \
      --arg ns "${SERVICE_NAMESPACE}" \
      --arg svc "${SERVICE_NAME}" \
      --argjson port "${SERVICE_PORT}" '
      .data[]?
      | select(.metadata.namespace == $ns)
      | select(.metadata.labels["kubernetes.io/service-name"] == $svc)
      | select(any(.ports[]?; (.port // 0) == $port))
      | .endpoints[]?
      | select((.conditions.ready // true) == true)
      | .addresses[]?
    ' | head -n 1
  )"
  if [[ -n "${ip_from_slice}" && "${ip_from_slice}" != "null" ]]; then
    echo "${ip_from_slice}"
    exit 0
  fi
fi

ep_json="$(curl -fsS "${GATEWAY_ADMIN_URL}/configclient/Endpoint/list" 2>/dev/null || true)"
if [[ -n "${ep_json}" ]]; then
  ip_from_ep="$(
    echo "${ep_json}" | jq -r \
      --arg ns "${SERVICE_NAMESPACE}" \
      --arg svc "${SERVICE_NAME}" \
      --argjson port "${SERVICE_PORT}" '
      .data[]?
      | select(.metadata.namespace == $ns and .metadata.name == $svc)
      | .subsets[]?
      | select(any(.ports[]?; (.port // 0) == $port))
      | .addresses[]?
      | .ip
    ' | head -n 1
  )"
  if [[ -n "${ip_from_ep}" && "${ip_from_ep}" != "null" ]]; then
    echo "${ip_from_ep}"
    exit 0
  fi
fi

exit 1
