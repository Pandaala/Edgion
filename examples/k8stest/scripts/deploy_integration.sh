#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/../edgion-deploy/kubernetes}"
TARGET_SCRIPT="$K8S_DEPLOY_ROOT/scripts/deploy_integration.sh"
VALIDATE_SCRIPT="$SCRIPT_DIR/validate_no_endpoints.sh"
TEST_SERVER_REPLICAS="${TEST_SERVER_REPLICAS:-3}"
BACKEND_TEST_NAMESPACE="${BACKEND_TEST_NAMESPACE:-edgion-backend}"
SCRIPT_ARGS=()

show_help() {
  cat <<EOF
Usage: $0 [options]

Options:
  --test-server-replicas <n>  Scale edgion-test-server deployments to n replicas (default: ${TEST_SERVER_REPLICAS})
  --skip-crd                  Pass through to deploy script
  --skip-test                 Pass through to deploy script
  --no-wait                   Pass through to deploy script
  --spec-profile <profile>    Pass through to deploy script
  -h, --help                  Show this help
EOF
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
    --skip-crd|--skip-test|--no-wait)
      SCRIPT_ARGS+=("$1")
      shift
      ;;
    --spec-profile)
      SCRIPT_ARGS+=("$1" "${2:-}")
      if [[ -z "${2:-}" ]]; then
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

if [[ ! -x "$TARGET_SCRIPT" ]]; then
  echo "deploy script not found or not executable: $TARGET_SCRIPT"
  echo "set K8S_DEPLOY_ROOT to your deploy repo kubernetes directory"
  exit 1
fi

if [[ ! -x "$VALIDATE_SCRIPT" ]]; then
  echo "validate script not found or not executable: $VALIDATE_SCRIPT"
  exit 1
fi

"$VALIDATE_SCRIPT" "$PROJECT_ROOT/examples/k8stest/conf"

"$TARGET_SCRIPT" "${SCRIPT_ARGS[@]}"

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

if [[ "${TEST_SERVER_REPLICAS}" -gt 0 ]]; then
  scale_if_exists "edgion-test" "edgion-test-server" "${TEST_SERVER_REPLICAS}"
  scale_if_exists "edgion-default" "edgion-test-server" "${TEST_SERVER_REPLICAS}"
  scale_if_exists "${BACKEND_TEST_NAMESPACE}" "edgion-test-server" "${TEST_SERVER_REPLICAS}"
fi

echo "Deploy integration finished."
