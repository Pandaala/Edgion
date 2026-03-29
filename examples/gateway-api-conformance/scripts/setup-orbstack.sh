#!/usr/bin/env bash
set -euo pipefail

# OrbStack-specific setup for Gateway API conformance testing.
# Builds Docker images locally and deploys to OrbStack K8s.
#
# Usage:
#   ./scripts/setup-orbstack.sh                # Full setup (build + deploy, debug)
#   ./scripts/setup-orbstack.sh --release      # Build release instead of debug
#   ./scripts/setup-orbstack.sh --skip-build   # Deploy only (reuse existing images)
#   ./scripts/setup-orbstack.sh --rebuild      # Force rebuild images (no cache)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMANCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT="$(cd "$CONFORMANCE_ROOT/../.." && pwd)"

CRD_ROOT="$PROJECT_ROOT/config/crd"
K8S_DEPLOY_ROOT="$PROJECT_ROOT/examples/k8stest/kubernetes"
MANIFESTS_DIR="$CONFORMANCE_ROOT/manifests"

SKIP_BUILD=false
NO_CACHE=""
PROFILE="debug"
CONTROLLER_REPLICAS="${CONTROLLER_REPLICAS:-1}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=true; shift ;;
    --rebuild)    NO_CACHE="--no-cache"; shift ;;
    --release)    PROFILE="release"; shift ;;
    -h|--help)
      echo "Usage: $0 [--skip-build] [--rebuild] [--release]"
      echo "  --skip-build  Skip Docker image build, use existing images"
      echo "  --rebuild     Force rebuild images without Docker cache"
      echo "  --release     Build release profile (default: debug)"
      exit 0 ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

TAG="conformance-dev"
CONTROLLER_IMAGE="pandaala/edgion-controller:${TAG}"
GATEWAY_IMAGE="pandaala/edgion-gateway:${TAG}"

echo "=== OrbStack Gateway API Conformance Setup ==="

# ------------------------------------------------------------------
# 0. Preflight checks
# ------------------------------------------------------------------
echo "[0] Preflight checks"

if ! kubectl config current-context 2>/dev/null | grep -q orbstack; then
  echo "WARNING: kubectl context is not 'orbstack'. Current: $(kubectl config current-context 2>/dev/null)"
  echo "         Run: kubectl config use-context orbstack"
  read -p "Continue anyway? [y/N] " -n 1 -r
  echo
  [[ $REPLY =~ ^[Yy]$ ]] || exit 1
fi

kubectl get nodes >/dev/null 2>&1 || { echo "ERROR: Cannot reach K8s cluster"; exit 1; }
echo "  K8s cluster reachable"

# ------------------------------------------------------------------
# 1. Build images (if not skipped)
# ------------------------------------------------------------------
if [[ "${SKIP_BUILD}" != "true" ]]; then
  echo "[1] Building Docker images (profile=${PROFILE})..."
  cd "$PROJECT_ROOT"

  echo "  Building controller..."
  docker build ${NO_CACHE} \
    --build-arg BINARY=edgion-controller \
    --build-arg BUILD_TYPE=bin \
    --build-arg PROFILE="${PROFILE}" \
    -t "${CONTROLLER_IMAGE}" \
    -f docker/Dockerfile .

  echo "  Building gateway..."
  docker build ${NO_CACHE} \
    --build-arg BINARY=edgion-gateway \
    --build-arg BUILD_TYPE=bin \
    --build-arg PROFILE="${PROFILE}" \
    -t "${GATEWAY_IMAGE}" \
    -f docker/Dockerfile .

  echo "  Images built: ${CONTROLLER_IMAGE}, ${GATEWAY_IMAGE}"
else
  echo "[1] Skipping image build (--skip-build)"
  docker image inspect "${CONTROLLER_IMAGE}" >/dev/null 2>&1 || { echo "ERROR: ${CONTROLLER_IMAGE} not found"; exit 1; }
  docker image inspect "${GATEWAY_IMAGE}" >/dev/null 2>&1 || { echo "ERROR: ${GATEWAY_IMAGE} not found"; exit 1; }
fi

cd "$CONFORMANCE_ROOT"

# ------------------------------------------------------------------
# 2. Namespace + CRDs
# ------------------------------------------------------------------
echo "[2] Apply namespaces and CRDs"
kubectl apply --server-side --force-conflicts -f "$K8S_DEPLOY_ROOT/namespace.yaml"

# Install Gateway API experimental CRDs (superset of standard).
# Clean existing CRDs + admission policy first to avoid safe-upgrades conflict.
kubectl delete validatingadmissionpolicy safe-upgrades.gateway.networking.k8s.io 2>/dev/null || true
kubectl delete validatingadmissionpolicybinding safe-upgrades.gateway.networking.k8s.io 2>/dev/null || true
for crd in $(kubectl get crd -o name 2>/dev/null | grep gateway.networking.k8s.io); do
  kubectl delete "$crd" 2>/dev/null || true
done
kubectl apply --server-side --force-conflicts -f "$CRD_ROOT/gateway-api/gateway-api-experimental-v1.5.0.yaml"

kubectl apply --server-side --force-conflicts -f "$CRD_ROOT/edgion-crd/"

# ------------------------------------------------------------------
# 3. Controller
# ------------------------------------------------------------------
echo "[3] Deploy controller"
kubectl apply -f "$K8S_DEPLOY_ROOT/controller/rbac.yaml"
kubectl apply -f "$K8S_DEPLOY_ROOT/controller/service.yaml"
kubectl apply -f "$K8S_DEPLOY_ROOT/controller/leader-service.yaml" 2>/dev/null || true
kubectl apply -f "$K8S_DEPLOY_ROOT/controller/rbac-leader-election.yaml" 2>/dev/null || true
kubectl apply -f "$MANIFESTS_DIR/controller-configmap.yaml"
kubectl apply -f "$K8S_DEPLOY_ROOT/controller/deployment.yaml"

# Override image, replicas, and inject RUST_BACKTRACE for debug builds
BACKTRACE_VAL=$([ "${PROFILE}" = "release" ] && echo "0" || echo "full")
kubectl -n edgion-system set image deployment/edgion-controller \
  edgion-controller="${CONTROLLER_IMAGE}"
kubectl -n edgion-system patch deployment edgion-controller \
  -p "{\"spec\":{\"replicas\":${CONTROLLER_REPLICAS},\"template\":{\"spec\":{\"containers\":[{\"name\":\"edgion-controller\",\"imagePullPolicy\":\"Never\",\"env\":[{\"name\":\"RUST_BACKTRACE\",\"value\":\"${BACKTRACE_VAL}\"}]}]}}}}"

echo "  Waiting for controller..."
kubectl rollout status deployment/edgion-controller -n edgion-system --timeout=120s

# ------------------------------------------------------------------
# 4. GatewayClass + EdgionGatewayConfig
# ------------------------------------------------------------------
echo "[4] Apply GatewayClass and EdgionGatewayConfig"
kubectl apply -f "$MANIFESTS_DIR/edgion-gateway-config.yaml"
kubectl apply -f "$MANIFESTS_DIR/gatewayclass.yaml"

# ------------------------------------------------------------------
# 5. Pre-create conformance Gateways
# ------------------------------------------------------------------
echo "[5] Pre-create conformance Gateways"
kubectl apply --server-side --force-conflicts -f "$MANIFESTS_DIR/conformance-gateway.yaml"
sleep 3

# ------------------------------------------------------------------
# 6. Gateway data plane
# ------------------------------------------------------------------
echo "[6] Deploy gateway"
kubectl apply -f "$K8S_DEPLOY_ROOT/gateway/service.yaml"
kubectl apply -f "$K8S_DEPLOY_ROOT/gateway/configmap.yaml"
kubectl apply -f "$K8S_DEPLOY_ROOT/gateway/deployment.yaml"

kubectl -n edgion-system set image deployment/edgion-gateway \
  edgion-gateway="${GATEWAY_IMAGE}"
kubectl -n edgion-system patch deployment edgion-gateway \
  -p "{\"spec\":{\"template\":{\"spec\":{\"containers\":[{\"name\":\"edgion-gateway\",\"imagePullPolicy\":\"Never\",\"env\":[{\"name\":\"RUST_BACKTRACE\",\"value\":\"${BACKTRACE_VAL}\"}]}]}}}}"

echo "  Restarting gateway to pick up pre-created listeners..."
kubectl rollout restart deployment/edgion-gateway -n edgion-system
kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=120s

# ------------------------------------------------------------------
# 7. Patch gateway_address
# ------------------------------------------------------------------
echo "[7] Patching gateway_address with Service ClusterIP"
GW_SVC_IP=$(kubectl -n edgion-system get svc edgion-gateway \
  -o jsonpath='{.spec.clusterIP}' 2>/dev/null || true)

if [[ -n "${GW_SVC_IP}" && "${GW_SVC_IP}" != "None" ]]; then
  echo "  Gateway Service ClusterIP: ${GW_SVC_IP}"

  CURRENT_TOML=$(kubectl -n edgion-system get configmap edgion-controller-config \
    -o jsonpath='{.data.edgion-controller\.toml}')

  UPDATED_TOML=$(echo "${CURRENT_TOML}" | sed "s/gateway_address = \"[^\"]*\"/gateway_address = \"${GW_SVC_IP}\"/")

  kubectl -n edgion-system create configmap edgion-controller-config \
    --from-literal="edgion-controller.toml=${UPDATED_TOML}" \
    --dry-run=client -o yaml | kubectl apply -f -

  kubectl rollout restart deployment/edgion-controller -n edgion-system
  kubectl rollout status deployment/edgion-controller -n edgion-system --timeout=120s
  sleep 3

  echo "  Waiting for Gateway status.addresses..."
  for i in $(seq 1 30); do
    ADDR=$(kubectl -n gateway-conformance-infra get gateway same-namespace \
      -o jsonpath='{.status.addresses[0].value}' 2>/dev/null || true)
    if [[ "${ADDR}" == "${GW_SVC_IP}" ]]; then
      echo "  Gateway address updated to ${GW_SVC_IP}"
      break
    fi
    [[ "$i" -eq 30 ]] && echo "WARNING: Gateway address not updated after 30s. Current: '${ADDR}'"
    sleep 1
  done
else
  echo "WARNING: Could not determine gateway Service ClusterIP"
fi

# ------------------------------------------------------------------
# 8. Verify
# ------------------------------------------------------------------
echo "[8] Verifying readiness"
for i in $(seq 1 30); do
  ACCEPTED=$(kubectl get gatewayclass edgion -o jsonpath='{.status.conditions[?(@.type=="Accepted")].status}' 2>/dev/null || true)
  if [[ "${ACCEPTED}" == "True" ]]; then
    echo "  GatewayClass 'edgion' Accepted"
    break
  fi
  [[ "$i" -eq 30 ]] && echo "WARNING: GatewayClass not Accepted after 30s"
  sleep 1
done

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Quick test:"
echo "  ./scripts/orbstack-dev.sh test --run-test HTTPRouteSimpleSameNamespace"
echo ""
echo "Hot-reload after code change:"
echo "  ./scripts/orbstack-dev.sh rebuild"
