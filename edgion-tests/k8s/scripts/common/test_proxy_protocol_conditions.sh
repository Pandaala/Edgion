#!/usr/bin/env bash
# Assert Kubernetes Gateway status for inbound PROXY protocol failures.
set -euo pipefail

NAMESPACE="${1:?usage: test_proxy_protocol_conditions.sh NAMESPACE GATEWAY_CLASS}"
GATEWAY_CLASS="${2:?usage: test_proxy_protocol_conditions.sh NAMESPACE GATEWAY_CLASS}"
KUBECTL_BIN="${KUBECTL_BIN:-kubectl}"
TRUST_NAMESPACE="${NAMESPACE}-pp-trust"

for tool in "$KUBECTL_BIN" jq; do
    command -v "$tool" >/dev/null 2>&1 || { echo "ERROR: missing tool $tool" >&2; exit 1; }
done

cleanup() {
    "$KUBECTL_BIN" delete gateway -n "$NAMESPACE" \
        pp-status-intrinsic pp-status-ref pp-status-shared-a pp-status-shared-b \
        --ignore-not-found --wait=false >/dev/null 2>&1 || true
}
trap cleanup EXIT

"$KUBECTL_BIN" get namespace "$TRUST_NAMESPACE" >/dev/null
"$KUBECTL_BIN" apply -f - >/dev/null <<EOF
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: denied-policy
  namespace: $TRUST_NAMESPACE
spec:
  enable: true
  visibility: Namespace
  data:
    type: ProxyProtocolTrust
    config: {mode: required}
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: pp-status-intrinsic
  namespace: $NAMESPACE
  annotations:
    edgion.io/inbound-proxy-protocol: '{'
spec:
  gatewayClassName: $GATEWAY_CLASS
  listeners: [{name: tcp, protocol: TCP, port: 59630}]
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: pp-status-ref
  namespace: $NAMESPACE
  annotations:
    edgion.io/inbound-proxy-protocol: '{"source":"resourceRef","resourceRef":{"name":"denied-policy","namespace":"$TRUST_NAMESPACE"}}'
spec:
  gatewayClassName: $GATEWAY_CLASS
  listeners: [{name: tcp, protocol: TCP, port: 59631}]
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: pp-status-shared-a
  namespace: $NAMESPACE
  annotations:
    edgion.io/inbound-proxy-protocol: '{"source":"inline","config":{"mode":"required"}}'
spec:
  gatewayClassName: $GATEWAY_CLASS
  listeners: [{name: tcp, protocol: TCP, port: 59632}]
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: pp-status-shared-b
  namespace: $NAMESPACE
  annotations:
    edgion.io/inbound-proxy-protocol: '{"source":"inline","config":{"mode":"trustedSources","trustedCidrs":["127.0.0.1"]}}'
spec:
  gatewayClassName: $GATEWAY_CLASS
  listeners: [{name: tcp, protocol: TCP, port: 59632}]
EOF

condition_matches() {
    local name="$1" scope="$2" type="$3" status="$4" reason="$5"
    "$KUBECTL_BIN" get gateway "$name" -n "$NAMESPACE" -o json 2>/dev/null | jq -e \
        --arg scope "$scope" --arg type "$type" --arg status "$status" --arg reason "$reason" '
        if $scope == "gateway" then .status.conditions
        else [.status.listeners[]? | select(.name == "tcp") | .conditions[]?] end
        | any(.[]?; .type == $type and .status == $status and .reason == $reason)
        ' >/dev/null
}

wait_condition() {
    local name="$1" scope="$2" type="$3" status="$4" reason="$5" i
    for i in $(seq 1 300); do
        condition_matches "$name" "$scope" "$type" "$status" "$reason" && return 0
        sleep 0.1
    done
    echo "ERROR: Gateway/$name missing $scope condition $type=$status/$reason" >&2
    "$KUBECTL_BIN" get gateway "$name" -n "$NAMESPACE" -o yaml >&2 || true
    return 1
}

wait_condition pp-status-intrinsic gateway Accepted False InvalidProxyProtocolConfig
wait_condition pp-status-intrinsic listener Accepted False InvalidProxyProtocolConfig
wait_condition pp-status-ref gateway ResolvedRefs False RefNotPermitted
wait_condition pp-status-ref listener ResolvedRefs False RefNotPermitted
for name in pp-status-shared-a pp-status-shared-b; do
    wait_condition "$name" gateway ListenersNotValid True ProxyProtocolPolicyConflict
    wait_condition "$name" listener Conflicted True ProxyProtocolPolicyConflict
done

echo "PASS: Kubernetes inbound PROXY protocol Gateway conditions"
