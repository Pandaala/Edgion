#!/usr/bin/env bash
set -euo pipefail

# Run only against an isolated v1.6.2 integration topology with its test
# Services and public-gateway class already installed. This entry verifies
# that both canonical v1 Route kinds attach and receive accepted status.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURE="${SCRIPT_DIR}/../conf/GatewayApiV16/routes.yaml"
NAMESPACE="edgion-test"
if [[ "${1:-}" != "--context" || -z "${2:-}" || -z "${KUBECONFIG:-}" ]]; then
  echo "usage: KUBECONFIG=<isolated-file> $0 --context <isolated-context>" >&2
  exit 1
fi
KUBE_CONTEXT="$2"
if [[ "$(command kubectl config current-context)" != "${KUBE_CONTEXT}" ]]; then
  echo "KUBECONFIG current context does not match ${KUBE_CONTEXT}" >&2
  exit 1
fi
kubectl() { command kubectl --context "${KUBE_CONTEXT}" "$@"; }

for resource in tcproutes.gateway.networking.k8s.io udproutes.gateway.networking.k8s.io; do
  bundle="$(kubectl get crd "${resource}" -o jsonpath='{.metadata.annotations.gateway\.networking\.k8s\.io/bundle-version}')"
  if [[ "${bundle}" != "v1.6.2" ]]; then
    echo "${resource}: expected v1.6.2 CRD bundle, got ${bundle}" >&2
    exit 1
  fi
done

kubectl apply -f "${FIXTURE}"
trap 'kubectl delete -f "${FIXTURE}" --ignore-not-found >/dev/null' EXIT
for kind in tcproute udproute; do
  if [[ "${kind}" == "tcproute" ]]; then
    name="v16-tcp-route"
  else
    name="v16-udp-route"
  fi
  for attempt in $(seq 1 30); do
    status="$(kubectl get "${kind}" "${name}" -n "${NAMESPACE}" -o jsonpath='{.status.parents[0].conditions[?(@.type=="Accepted")].status}' 2>/dev/null || true)"
    if [[ "${status}" == "True" ]]; then break; fi
    if [[ "${attempt}" == 30 ]]; then
      kubectl get "${kind}" "${name}" -n "${NAMESPACE}" -o yaml
      echo "${kind}/${name} was not accepted" >&2
      exit 1
    fi
    sleep 2
  done
  echo "${kind}/${name}: Accepted=True"
done
