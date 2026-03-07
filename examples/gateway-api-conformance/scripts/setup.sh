#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMANCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT="$(cd "$CONFORMANCE_ROOT/../.." && pwd)"

CRD_ROOT="${CRD_ROOT:-$PROJECT_ROOT/config/crd}"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/examples/k8stest/kubernetes}"
MANIFESTS_DIR="$CONFORMANCE_ROOT/manifests"

CONTROLLER_IMAGE="${CONTROLLER_IMAGE:-}"
GATEWAY_IMAGE="${GATEWAY_IMAGE:-}"
PULL_POLICY="${PULL_POLICY:-Always}"
SKIP_CRD="${SKIP_CRD:-false}"

show_help() {
  cat <<USAGE
Usage: $0 [options]

Deploy Edgion and set up the cluster for Gateway API conformance testing.

Options:
  --controller-image <img>  Override controller image
  --gateway-image <img>     Override gateway image
  --pull-policy <policy>    Image pull policy (default: Always)
  --skip-crd                Skip CRD installation
  -h, --help                Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --controller-image) CONTROLLER_IMAGE="$2"; shift 2 ;;
    --gateway-image)    GATEWAY_IMAGE="$2";    shift 2 ;;
    --pull-policy)      PULL_POLICY="$2";      shift 2 ;;
    --skip-crd)         SKIP_CRD=true;         shift   ;;
    -h|--help)          show_help; exit 0              ;;
    *) echo "Unknown option: $1"; show_help; exit 1    ;;
  esac
done

echo "=== Gateway API Conformance Setup ==="

# ------------------------------------------------------------------
# 1. Namespace
# ------------------------------------------------------------------
echo "[1/7] Apply edgion-system namespace"
kubectl apply --server-side --force-conflicts -f "$K8S_DEPLOY_ROOT/namespace.yaml"

# ------------------------------------------------------------------
# 2. CRDs
# ------------------------------------------------------------------
if [[ "${SKIP_CRD}" != "true" ]]; then
  echo "[2/7] Apply Gateway API CRDs"
  kubectl apply -f "$CRD_ROOT/gateway-api/"
  echo "       Apply Edgion CRDs"
  kubectl apply -f "$CRD_ROOT/edgion-crd/"
else
  echo "[2/7] Skip CRD apply"
fi

# ------------------------------------------------------------------
# 3. Controller (with conformance ConfigMap)
# ------------------------------------------------------------------
echo "[3/8] Apply controller"
kubectl apply -f "$K8S_DEPLOY_ROOT/controller/rbac.yaml"
kubectl apply -f "$K8S_DEPLOY_ROOT/controller/service.yaml"
kubectl apply -f "$MANIFESTS_DIR/controller-configmap.yaml"
kubectl apply -f "$K8S_DEPLOY_ROOT/controller/deployment.yaml"
if [[ -n "${CONTROLLER_IMAGE}" ]]; then
  echo "       Override controller image: ${CONTROLLER_IMAGE}"
  kubectl set image deployment/edgion-controller -n edgion-system \
    edgion-controller="${CONTROLLER_IMAGE}"
  kubectl patch deployment edgion-controller -n edgion-system \
    -p "{\"spec\":{\"template\":{\"spec\":{\"containers\":[{\"name\":\"edgion-controller\",\"imagePullPolicy\":\"${PULL_POLICY}\"}]}}}}"
fi

echo "       Waiting for controller to be ready..."
kubectl rollout status deployment/edgion-controller -n edgion-system --timeout=120s

# ------------------------------------------------------------------
# 4. GatewayClass + EdgionGatewayConfig
# ------------------------------------------------------------------
echo "[4/8] Apply GatewayClass 'edgion' and EdgionGatewayConfig"
kubectl apply -f "$MANIFESTS_DIR/edgion-gateway-config.yaml"
kubectl apply -f "$MANIFESTS_DIR/gatewayclass.yaml"

# ------------------------------------------------------------------
# 5. Pre-create conformance namespaces and Gateway CRs
# ------------------------------------------------------------------
echo "[5/8] Pre-create conformance base Gateways (listener workaround)"
kubectl apply --server-side --force-conflicts -f "$MANIFESTS_DIR/conformance-gateway.yaml"

# Wait for controller to process the Gateways
sleep 3

# ------------------------------------------------------------------
# 6. Gateway data plane
# ------------------------------------------------------------------
echo "[6/8] Apply gateway"
kubectl apply -f "$K8S_DEPLOY_ROOT/gateway/service.yaml"
kubectl apply -f "$K8S_DEPLOY_ROOT/gateway/configmap.yaml"
kubectl apply -f "$K8S_DEPLOY_ROOT/gateway/deployment.yaml"
if [[ -n "${GATEWAY_IMAGE}" ]]; then
  echo "       Override gateway image: ${GATEWAY_IMAGE}"
  kubectl set image deployment/edgion-gateway -n edgion-system \
    edgion-gateway="${GATEWAY_IMAGE}"
  kubectl patch deployment edgion-gateway -n edgion-system \
    -p "{\"spec\":{\"template\":{\"spec\":{\"containers\":[{\"name\":\"edgion-gateway\",\"imagePullPolicy\":\"${PULL_POLICY}\"}]}}}}"
fi

echo "       Restarting gateway to pick up pre-created listeners..."
kubectl rollout restart deployment/edgion-gateway -n edgion-system
kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=120s

# ------------------------------------------------------------------
# 7. Patch gateway_address with actual Service ClusterIP
# ------------------------------------------------------------------
echo "[7/8] Patching Gateway addresses"
GW_SVC_IP=$(kubectl -n edgion-system get svc edgion-gateway \
  -o jsonpath='{.spec.clusterIP}' 2>/dev/null || true)

if [[ -n "${GW_SVC_IP}" && "${GW_SVC_IP}" != "None" ]]; then
  echo "       Gateway Service ClusterIP: ${GW_SVC_IP}"

  # Update controller configmap with actual gateway_address
  CURRENT_TOML=$(kubectl -n edgion-system get configmap edgion-controller-config \
    -o jsonpath='{.data.edgion-controller\.toml}')

  UPDATED_TOML=$(echo "${CURRENT_TOML}" | sed "s/gateway_address = \"[^\"]*\"/gateway_address = \"${GW_SVC_IP}\"/")

  kubectl -n edgion-system create configmap edgion-controller-config \
    --from-literal="edgion-controller.toml=${UPDATED_TOML}" \
    --dry-run=client -o yaml | kubectl apply -f -

  # Restart controller to pick up new config
  kubectl rollout restart deployment/edgion-controller -n edgion-system
  kubectl rollout status deployment/edgion-controller -n edgion-system --timeout=120s
  sleep 3

  # Wait for controller to re-process Gateways with correct address
  echo "       Waiting for Gateway status.addresses to be updated..."
  for i in $(seq 1 30); do
    ADDR=$(kubectl -n gateway-conformance-infra get gateway same-namespace \
      -o jsonpath='{.status.addresses[0].value}' 2>/dev/null || true)
    if [[ "${ADDR}" == "${GW_SVC_IP}" ]]; then
      echo "       Gateway address updated to ${GW_SVC_IP}"
      break
    fi
    if [[ "$i" -eq 30 ]]; then
      echo "WARNING: Gateway address not updated after 30s. Current: '${ADDR}'"
    fi
    sleep 1
  done
else
  echo "WARNING: Could not determine gateway Service ClusterIP. Status addresses may be wrong."
fi

# ------------------------------------------------------------------
# 8. Verify readiness
# ------------------------------------------------------------------
echo "[8/8] Verifying readiness"
echo "       Checking GatewayClass status..."
for i in $(seq 1 30); do
  ACCEPTED=$(kubectl get gatewayclass edgion -o jsonpath='{.status.conditions[?(@.type=="Accepted")].status}' 2>/dev/null || true)
  if [[ "${ACCEPTED}" == "True" ]]; then
    echo "       GatewayClass 'edgion' is Accepted."
    break
  fi
  if [[ "$i" -eq 30 ]]; then
    echo "WARNING: GatewayClass 'edgion' not yet Accepted after 30s. Check controller logs."
  fi
  sleep 1
done

echo ""
echo "=== Setup Complete ==="
echo "Run ./scripts/run.sh to execute conformance tests."
