#!/usr/bin/env bash
set -euo pipefail

echo "=== Gateway API Conformance Cleanup ==="

echo "[1/4] Delete conformance namespaces"
for ns in gateway-conformance-infra gateway-conformance-app-backend gateway-conformance-web-backend; do
  kubectl delete namespace "$ns" --ignore-not-found=true --wait=false
done

echo "[2/4] Delete GatewayClass 'edgion' and EdgionGatewayConfig"
kubectl delete gatewayclass edgion --ignore-not-found=true
kubectl delete edgiongatewayconfiguration conformance-gateway --ignore-not-found=true 2>/dev/null || true

echo "[3/4] Delete Edgion deployments (controller + gateway)"
kubectl delete deployment edgion-controller -n edgion-system --ignore-not-found=true
kubectl delete deployment edgion-gateway -n edgion-system --ignore-not-found=true
kubectl delete service edgion-controller -n edgion-system --ignore-not-found=true
kubectl delete service edgion-gateway -n edgion-system --ignore-not-found=true
kubectl delete configmap edgion-controller-config -n edgion-system --ignore-not-found=true
kubectl delete configmap edgion-gateway-config -n edgion-system --ignore-not-found=true

echo "[4/4] Waiting for namespaces to be deleted..."
for ns in gateway-conformance-infra gateway-conformance-app-backend gateway-conformance-web-backend; do
  kubectl wait --for=delete namespace/"$ns" --timeout=60s 2>/dev/null || true
done

echo ""
echo "=== Cleanup Complete ==="
echo "Note: CRDs and edgion-system namespace are kept. Delete manually if needed:"
echo "  kubectl delete namespace edgion-system"
echo "  kubectl delete -f config/crd/gateway-api/"
echo "  kubectl delete -f config/crd/edgion-crd/"
