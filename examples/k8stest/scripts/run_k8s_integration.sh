#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
K8S_DEPLOY_ROOT="${K8S_DEPLOY_ROOT:-$PROJECT_ROOT/../edgion-deploy/kubernetes}"
TARGET_SCRIPT="$K8S_DEPLOY_ROOT/scripts/run_k8s_integration.sh"

if [[ ! -x "$TARGET_SCRIPT" ]]; then
  echo "run script not found or not executable: $TARGET_SCRIPT"
  echo "set K8S_DEPLOY_ROOT to your deploy repo kubernetes directory"
  exit 1
fi

exec "$TARGET_SCRIPT" "$PROJECT_ROOT" "$@"
