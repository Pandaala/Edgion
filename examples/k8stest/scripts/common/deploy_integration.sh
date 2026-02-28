#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/examples/k8stest/kubernetes}"
CRD_ROOT="${CRD_ROOT:-$PROJECT_ROOT/config/crd}"
VALIDATE_SCRIPT="$SCRIPT_DIR/validate_no_endpoints.sh"
TEST_SERVER_REPLICAS="${TEST_SERVER_REPLICAS:-3}"
BACKEND_TEST_NAMESPACE="${BACKEND_TEST_NAMESPACE:-edgion-backend}"

SKIP_CRD=false
SKIP_TEST=false
WAIT_READY=true
SPEC_PROFILE=""

CONTROLLER_IMAGE="${CONTROLLER_IMAGE:-}"
GATEWAY_IMAGE="${GATEWAY_IMAGE:-}"
TEST_CLIENT_IMAGE="${TEST_CLIENT_IMAGE:-}"
TEST_SERVER_IMAGE="${TEST_SERVER_IMAGE:-}"
PULL_POLICY_OVERRIDE="${PULL_POLICY_OVERRIDE:-}"

show_help() {
  cat <<USAGE
Usage: $0 [options]

Options:
  --test-server-replicas <n>  Scale edgion-test-server deployments to n replicas (default: ${TEST_SERVER_REPLICAS})
  --skip-crd                  Skip CRD apply (CRD source: ${CRD_ROOT})
  --skip-test                 Skip test workload apply
  --no-wait                   Do not wait for rollout ready
  --spec-profile <profile>    Compatibility no-op (profiles are no longer used)
  -h, --help                  Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --test-server-replicas)
      TEST_SERVER_REPLICAS="${2:-}"
      if ! [[ "${TEST_SERVER_REPLICAS}" =~ ^[0-9]+$ ]]; then
        echo "invalid --test-server-replicas: ${TEST_SERVER_REPLICAS}"
        exit 1
      fi
      shift 2
      ;;
    --skip-crd)
      SKIP_CRD=true
      shift
      ;;
    --skip-test)
      SKIP_TEST=true
      shift
      ;;
    --no-wait)
      WAIT_READY=false
      shift
      ;;
    --spec-profile)
      SPEC_PROFILE="${2:-}"
      if [[ -z "${SPEC_PROFILE}" ]]; then
        echo "missing value for --spec-profile"
        exit 1
      fi
      shift 2
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      echo "unknown argument: $1"
      show_help
      exit 1
      ;;
  esac
done

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH"
  exit 1
fi

if [[ ! -x "$VALIDATE_SCRIPT" ]]; then
  echo "validate script not found or not executable: $VALIDATE_SCRIPT"
  exit 1
fi

if [[ ! -f "$K8S_DEPLOY_ROOT/namespace.yaml" ]]; then
  echo "namespace manifest not found: $K8S_DEPLOY_ROOT/namespace.yaml"
  exit 1
fi
if [[ ! -d "$K8S_DEPLOY_ROOT/controller" ]]; then
  echo "controller manifest dir not found: $K8S_DEPLOY_ROOT/controller"
  exit 1
fi
if [[ ! -d "$K8S_DEPLOY_ROOT/gateway" ]]; then
  echo "gateway manifest dir not found: $K8S_DEPLOY_ROOT/gateway"
  exit 1
fi
if [[ ! -d "$K8S_DEPLOY_ROOT/test" ]]; then
  echo "test manifest dir not found: $K8S_DEPLOY_ROOT/test"
  exit 1
fi

if [[ "${SKIP_CRD}" != "true" ]]; then
  if [[ ! -d "$CRD_ROOT/gateway-api" || ! -d "$CRD_ROOT/edgion-crd" ]]; then
    echo "CRD directories not found under: $CRD_ROOT"
    exit 1
  fi
fi

"$VALIDATE_SCRIPT" "$PROJECT_ROOT/examples/k8stest/conf"

echo "[1/5] Apply namespaces"
kubectl apply --server-side --force-conflicts -f "$K8S_DEPLOY_ROOT/namespace.yaml"

if [[ "${SKIP_CRD}" != "true" ]]; then
  echo "[2/5] Apply Gateway API and Edgion CRDs from $CRD_ROOT"
  kubectl apply -f "$CRD_ROOT/gateway-api/"
  kubectl apply -f "$CRD_ROOT/edgion-crd/"
else
  echo "[2/5] Skip CRD apply"
fi

echo "[3/5] Apply controller"
kubectl apply -f "$K8S_DEPLOY_ROOT/controller/"
if [[ -n "${CONTROLLER_IMAGE}" ]]; then
  echo "Override controller image: ${CONTROLLER_IMAGE}"
  kubectl set image deployment/edgion-controller -n edgion-system edgion-controller="${CONTROLLER_IMAGE}"
fi

echo "[4/5] Apply gateway"
kubectl apply -f "$K8S_DEPLOY_ROOT/gateway/"
if [[ -n "${GATEWAY_IMAGE}" ]]; then
  echo "Override gateway image: ${GATEWAY_IMAGE}"
  kubectl set image deployment/edgion-gateway -n edgion-system edgion-gateway="${GATEWAY_IMAGE}"
fi
if [[ -n "${SPEC_PROFILE}" ]]; then
  echo "Ignore --spec-profile=${SPEC_PROFILE}: profiles have been removed from k8stest."
fi

if [[ "${SKIP_TEST}" != "true" ]]; then
  echo "[5/5] Apply test workloads"
  set +e
  APPLY_OUT="$(kubectl apply -k "$K8S_DEPLOY_ROOT/test/" 2>&1)"
  APPLY_RC=$?
  set -e

  if [[ "${APPLY_RC}" -ne 0 ]]; then
    echo "${APPLY_OUT}"
    if echo "${APPLY_OUT}" | grep -q 'Deployment.apps .* is invalid: spec.selector: .* field is immutable\|Deployment ".*" is invalid: spec.selector: .* field is immutable'; then
      echo "Detected immutable Deployment selector change in test workloads, recreating test deployments..."
      kubectl delete deployment edgion-test-server -n edgion-test --ignore-not-found=true
      kubectl delete deployment edgion-test-server -n edgion-default --ignore-not-found=true
      kubectl delete deployment edgion-test-server -n "${BACKEND_TEST_NAMESPACE}" --ignore-not-found=true
      kubectl delete deployment edgion-test-client -n edgion-test --ignore-not-found=true
      kubectl apply -k "$K8S_DEPLOY_ROOT/test/"
    else
      echo "Apply test workloads failed."
      exit "${APPLY_RC}"
    fi
  else
    echo "${APPLY_OUT}"
  fi

  if [[ -n "${TEST_SERVER_IMAGE}" ]]; then
    echo "Override test-server image: ${TEST_SERVER_IMAGE}"
    kubectl set image deployment/edgion-test-server -n edgion-test test-server="${TEST_SERVER_IMAGE}"
    kubectl set image deployment/edgion-test-server -n edgion-default test-server="${TEST_SERVER_IMAGE}"
    kubectl set image deployment/edgion-test-server -n "${BACKEND_TEST_NAMESPACE}" test-server="${TEST_SERVER_IMAGE}" || true
  fi

  if [[ -n "${TEST_CLIENT_IMAGE}" ]]; then
    echo "Override test-client image: ${TEST_CLIENT_IMAGE}"
    kubectl set image deployment/edgion-test-client -n edgion-test test-client="${TEST_CLIENT_IMAGE}"
  fi
else
  echo "[5/5] Skip test workloads"
fi

rollout_if_exists() {
  local ns="$1"
  local name="$2"
  local timeout_sec="${3:-300s}"

  if kubectl get deployment "${name}" -n "${ns}" >/dev/null 2>&1; then
    kubectl rollout status deployment/"${name}" -n "${ns}" --timeout="${timeout_sec}"
  else
    echo "Skip rollout check for missing deployment ${ns}/${name}"
  fi
}

set_pull_policy_if_exists() {
  local ns="$1"
  local name="$2"
  local policy="$3"
  if kubectl get deployment "${name}" -n "${ns}" >/dev/null 2>&1; then
    kubectl patch deployment "${name}" -n "${ns}" --type='json' \
      -p="[{\"op\":\"replace\",\"path\":\"/spec/template/spec/containers/0/imagePullPolicy\",\"value\":\"${policy}\"}]" \
      >/dev/null 2>&1 || true
  fi
}

if [[ -n "${PULL_POLICY_OVERRIDE}" ]]; then
  echo "Override deployment imagePullPolicy: ${PULL_POLICY_OVERRIDE}"
  set_pull_policy_if_exists "edgion-system" "edgion-controller" "${PULL_POLICY_OVERRIDE}"
  set_pull_policy_if_exists "edgion-system" "edgion-gateway" "${PULL_POLICY_OVERRIDE}"
  set_pull_policy_if_exists "edgion-test" "edgion-test-client" "${PULL_POLICY_OVERRIDE}"
  if [[ "${SKIP_TEST}" != "true" ]]; then
    set_pull_policy_if_exists "edgion-test" "edgion-test-server" "${PULL_POLICY_OVERRIDE}"
    set_pull_policy_if_exists "edgion-default" "edgion-test-server" "${PULL_POLICY_OVERRIDE}"
    set_pull_policy_if_exists "${BACKEND_TEST_NAMESPACE}" "edgion-test-server" "${PULL_POLICY_OVERRIDE}"
  fi
fi

if [[ "${WAIT_READY}" == "true" ]]; then
  echo "Waiting controller rollout..."
  rollout_if_exists "edgion-system" "edgion-controller" "300s"

  echo "Waiting gateway rollout..."
  rollout_if_exists "edgion-system" "edgion-gateway" "300s"

  if [[ "${SKIP_TEST}" != "true" ]]; then
    echo "Waiting test-server rollout (edgion-test)..."
    rollout_if_exists "edgion-test" "edgion-test-server" "300s"

    echo "Waiting test-server rollout (edgion-default)..."
    rollout_if_exists "edgion-default" "edgion-test-server" "300s"

    echo "Waiting test-server rollout (${BACKEND_TEST_NAMESPACE})..."
    rollout_if_exists "${BACKEND_TEST_NAMESPACE}" "edgion-test-server" "300s"

    echo "Waiting test-client rollout..."
    rollout_if_exists "edgion-test" "edgion-test-client" "300s"
  fi
fi

scale_if_exists() {
  local ns="$1"
  local name="$2"
  local replicas="$3"

  if kubectl get deployment "${name}" -n "${ns}" >/dev/null 2>&1; then
    echo "Scaling ${ns}/${name} to ${replicas}"
    kubectl scale deployment "${name}" -n "${ns}" --replicas="${replicas}"
    kubectl rollout status deployment/"${name}" -n "${ns}" --timeout=300s
  else
    echo "Skip scaling missing deployment ${ns}/${name}"
  fi
}

if [[ "${SKIP_TEST}" != "true" && "${TEST_SERVER_REPLICAS}" -gt 0 ]]; then
  scale_if_exists "edgion-test" "edgion-test-server" "${TEST_SERVER_REPLICAS}"
  scale_if_exists "edgion-default" "edgion-test-server" "${TEST_SERVER_REPLICAS}"
  scale_if_exists "${BACKEND_TEST_NAMESPACE}" "edgion-test-server" "${TEST_SERVER_REPLICAS}"
fi

echo "Deploy integration finished."
