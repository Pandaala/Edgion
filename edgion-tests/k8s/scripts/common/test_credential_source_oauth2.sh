#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/conf}"
CONF_DIR="$CONF_ROOT/LinkSys/CredentialSourceOAuth2"
NAMESPACE="edgion-default"
SOURCE="oauth2-credential-source"
PLUGIN="oauth2-credential-plugin"
ACTIVE_BOOTSTRAP="oauth2-bootstrap-active"
PREVIOUS_BOOTSTRAP="oauth2-bootstrap-previous"

cleanup() {
  kubectl delete edgionplugins "$PLUGIN" -n "$NAMESPACE" --ignore-not-found=true >/dev/null 2>&1 || true
  kubectl delete linksys "$SOURCE" -n "$NAMESPACE" --ignore-not-found=true >/dev/null 2>&1 || true
  kubectl delete secret "$ACTIVE_BOOTSTRAP" "$PREVIOUS_BOOTSTRAP" -n "$NAMESPACE" \
    --ignore-not-found=true >/dev/null 2>&1 || true
  kubectl delete secret "$PLUGIN" -n "$NAMESPACE" --ignore-not-found=true >/dev/null 2>&1 || true
}
trap cleanup EXIT

read_token() {
  kubectl get secret "$PLUGIN" -n "$NAMESPACE" -o json 2>/dev/null \
    | jq -r '.data.bearerToken // empty | @base64d' 2>/dev/null || true
}

wait_for_token_prefix() {
  local prefix="$1" timeout="$2" deadline token
  deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    token="$(read_token)"
    if [[ "$token" == "${prefix}"* ]]; then
      WAITED_TOKEN="$token"
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for managed Secret token prefix '${prefix}'" >&2
  return 1
}

wait_for_token_change() {
  local previous="$1" timeout="$2" deadline token
  deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    token="$(read_token)"
    if [[ -n "$token" && "$token" != "$previous" && "$token" == active-oauth2-token-* ]]; then
      WAITED_TOKEN="$token"
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for proactive OAuth2 token refresh" >&2
  return 1
}

wait_for_secret_absence() {
  local timeout="$1" deadline
  deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    if ! kubectl get secret "$PLUGIN" -n "$NAMESPACE" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for managed Secret deletion" >&2
  return 1
}

assert_managed_secret_identity() {
  local plugin_uid
  plugin_uid="$(kubectl get edgionplugins "$PLUGIN" -n "$NAMESPACE" -o jsonpath='{.metadata.uid}')"
  kubectl get secret "$PLUGIN" -n "$NAMESPACE" -o json | jq -e \
    --arg plugin "$PLUGIN" --arg plugin_uid "$plugin_uid" --arg source "$NAMESPACE/$SOURCE" '
      .metadata.name == $plugin
      and .metadata.labels["edgion.io/managed-by"] == "oauth2-credential"
      and .metadata.annotations["edgion.io/credential-source-name"] == $source
      and (.metadata.ownerReferences | length) == 1
      and .metadata.ownerReferences[0].apiVersion == "edgion.io/v1"
      and .metadata.ownerReferences[0].kind == "EdgionPlugins"
      and .metadata.ownerReferences[0].name == $plugin
      and .metadata.ownerReferences[0].uid == $plugin_uid
      and .metadata.ownerReferences[0].controller == true
    ' >/dev/null
}

cleanup
kubectl apply -f "$CONF_DIR/00_Secret_bootstrap.yaml"
kubectl apply -f "$CONF_DIR/01_LinkSys_credential-source.yaml"
kubectl apply -f "$CONF_DIR/02_EdgionPlugins_credential-source.yaml"

echo "[credential-source] verify previous bootstrap fallback"
WAITED_TOKEN=""
wait_for_token_prefix "previous-oauth2-token-" 90
previous_token="$WAITED_TOKEN"
assert_managed_secret_identity

echo "[credential-source] rotate to the active bootstrap"
kubectl patch secret "$ACTIVE_BOOTSTRAP" -n "$NAMESPACE" --type merge \
  -p '{"stringData":{"clientId":"oauth2-active-client","clientSecret":"active-secret"}}' >/dev/null
wait_for_token_prefix "active-oauth2-token-" 30
active_token="$WAITED_TOKEN"
if [[ "$active_token" == "$previous_token" ]]; then
  echo "Active bootstrap did not replace the previous-bootstrap token" >&2
  exit 1
fi

echo "[credential-source] verify expiry-aware proactive refresh"
wait_for_token_change "$active_token" 45
rotated_token="$WAITED_TOKEN"

echo "[credential-source] verify deleted output Secret self-heals"
old_uid="$(kubectl get secret "$PLUGIN" -n "$NAMESPACE" -o jsonpath='{.metadata.uid}')"
kubectl delete secret "$PLUGIN" -n "$NAMESPACE" --wait=true >/dev/null
wait_for_token_prefix "active-oauth2-token-" 20
new_uid="$(kubectl get secret "$PLUGIN" -n "$NAMESPACE" -o jsonpath='{.metadata.uid}')"
if [[ "$new_uid" == "$old_uid" ]]; then
  echo "Managed Secret was not recreated with a new Kubernetes UID" >&2
  exit 1
fi
assert_managed_secret_identity
if [[ -z "$rotated_token" ]]; then
  echo "Rotated token observation was unexpectedly empty" >&2
  exit 1
fi

echo "[credential-source] verify consumer deletion removes the output Secret"
kubectl delete edgionplugins "$PLUGIN" -n "$NAMESPACE" --wait=true >/dev/null
wait_for_secret_absence 20

echo "CredentialSource OAuth2 lifecycle integration passed"
