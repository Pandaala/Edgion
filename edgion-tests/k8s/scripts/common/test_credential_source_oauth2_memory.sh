#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/conf}"
CONF_DIR="$CONF_ROOT/LinkSys/CredentialSourceOAuth2Memory"
NAMESPACE="edgion-default"
SOURCE="oauth2-memory-credential-source"
PLUGIN="oauth2-memory-credential-plugin"
ROUTE="oauth2-memory-credential-route"
ACTIVE_BOOTSTRAP="oauth2-memory-bootstrap-active"
PREVIOUS_BOOTSTRAP="oauth2-memory-bootstrap-previous"
ROUTE_HOST="oauth2-memory.example.com"
GATEWAY_ECHO_URL="http://edgion-gateway.edgion-system.svc.cluster.local:31180/echo"
CLIENT_NAMESPACE="edgion-test"
CLIENT_SELECTOR="app=edgion-test-client"
CLIENT_CONTAINER="test-client"

cleanup() {
  kubectl delete httproute "$ROUTE" -n "$NAMESPACE" --ignore-not-found=true >/dev/null 2>&1 || true
  kubectl delete edgionplugins "$PLUGIN" -n "$NAMESPACE" --ignore-not-found=true >/dev/null 2>&1 || true
  kubectl delete linksys "$SOURCE" -n "$NAMESPACE" --ignore-not-found=true >/dev/null 2>&1 || true
  kubectl delete secret "$ACTIVE_BOOTSTRAP" "$PREVIOUS_BOOTSTRAP" -n "$NAMESPACE" \
    --ignore-not-found=true >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_for_credential_valid() {
  local timeout="$1" deadline state
  deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    state="$(kubectl get linksys "$SOURCE" -n "$NAMESPACE" \
      -o jsonpath='{.status.credentialState}' 2>/dev/null || true)"
    if [[ "$state" == "Valid" ]]; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for LinkSys status.credentialState=Valid (last='${state:-}')" >&2
  return 1
}

assert_generated_secret_absent() {
  if kubectl get secret "$PLUGIN" -n "$NAMESPACE" >/dev/null 2>&1; then
    echo "persist=false must not create generated Secret ${PLUGIN}" >&2
    return 1
  fi
}

test_client_pod() {
  kubectl get pods -n "$CLIENT_NAMESPACE" -l "$CLIENT_SELECTOR" \
    --field-selector=status.phase=Running \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true
}

ensure_curl_in_pod() {
  local pod="$1"
  kubectl exec -n "$CLIENT_NAMESPACE" -c "$CLIENT_CONTAINER" "$pod" -- sh -c \
    'command -v curl >/dev/null 2>&1 || (apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends curl ca-certificates >/dev/null)'
}

# Probe injection without printing the bearer token. The /echo body lists inbound
# headers; only the process exit code is observed on the host.
probe_injection() {
  local pod="$1"
  kubectl exec -i -n "$CLIENT_NAMESPACE" -c "$CLIENT_CONTAINER" "$pod" -- sh <<EOF >/dev/null
tmp=\$(mktemp)
code=\$(curl -sS --max-time 5 -o "\$tmp" -w "%{http_code}" \
  -H "Host: ${ROUTE_HOST}" \
  "${GATEWAY_ECHO_URL}" || printf "%s" "000")
line=\$(grep -i '^[[:space:]]*authorization:' "\$tmp" | head -n1 || true)
rm -f "\$tmp"
auth=\${line#*:}
auth=\$(printf "%s" "\$auth" | sed 's/^[[:space:]]*//')
rest=\${auth#Bearer }
if [ "\$code" = "200" ] && [ "\$rest" != "\$auth" ] && [ -n "\$rest" ]; then
  exit 0
fi
exit 1
EOF
}

wait_for_injection() {
  local timeout="$1" pod deadline
  pod="$(test_client_pod)"
  if [[ -z "$pod" ]]; then
    echo "No running ${CLIENT_SELECTOR} pod in ${CLIENT_NAMESPACE}" >&2
    return 1
  fi
  ensure_curl_in_pod "$pod"
  deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    if probe_injection "$pod"; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for HTTP 200 with an upstream Authorization Bearer prefix" >&2
  return 1
}

cleanup
kubectl apply -f "$CONF_DIR/00_Secret_bootstrap.yaml"
kubectl apply -f "$CONF_DIR/01_LinkSys_credential-source.yaml"
kubectl apply -f "$CONF_DIR/02_EdgionPlugins_credential-source.yaml"
kubectl apply -f "$CONF_DIR/03_HTTPRoute_credential-source.yaml"

echo "[credential-source-memory] wait for LinkSys status.credentialState=Valid"
wait_for_credential_valid 90

echo "[credential-source-memory] generated Secret must stay NotFound"
assert_generated_secret_absent

echo "[credential-source-memory] prove bearer injection via gateway /echo"
wait_for_injection 90
assert_generated_secret_absent

echo "CredentialSource OAuth2 persist-off memory publication passed"
