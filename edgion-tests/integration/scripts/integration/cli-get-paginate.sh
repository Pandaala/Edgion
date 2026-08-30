#!/usr/bin/env bash
# =============================================================================
# cli-get-paginate.sh
#
# Verifies `edgion-cli get httproute --limit 10 --all` returns 25 unique
# names across pages when the cluster has 25 HTTPRoute fixtures (Card 05).
#
# Requires K8s mode + a running edgion-controller at ADMIN_URL (default 12101).
# Self-SKIPs if kubectl, jq, the binary, or the controller is unavailable.
# =============================================================================
set -euo pipefail

command -v jq      >/dev/null || { echo "SKIP: jq required"; exit 0; }
command -v kubectl >/dev/null || { echo "SKIP: kubectl required"; exit 0; }
command -v python3 >/dev/null || { echo "SKIP: python3 required"; exit 0; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../utils/shell_compat.sh
source "${SCRIPT_DIR}/../utils/shell_compat.sh"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
CLI="${PROJECT_ROOT}/target/debug/edgion-cli"
ADMIN_URL="${ADMIN_URL:-http://127.0.0.1:12101}"
# /health lives on the dedicated probe listener, not the admin API.
PROBE_URL="${PROBE_URL:-http://127.0.0.1:12100}"

[[ -x "$CLI" ]] || { echo "SKIP: edgion-cli not built"; exit 0; }

kubectl get crd httproutes.gateway.networking.k8s.io >/dev/null 2>&1 \
    || { echo "SKIP: Gateway API CRDs not installed"; exit 0; }

curl -sf --max-time 3 "${PROBE_URL}/health" >/dev/null 2>&1 \
    || { echo "SKIP: controller not reachable at $PROBE_URL"; exit 0; }

# Authenticate: the test assumes a token is already saved in ~/.edgion/token
# (e.g., from a prior `edgion-cli login`). If not present, skip gracefully.
TOKEN_FILE="${HOME}/.edgion/token"
if [[ ! -s "$TOKEN_FILE" ]]; then
    echo "SKIP: no saved token at $TOKEN_FILE; run 'edgion-cli login' first"
    exit 0
fi

NAMESPACE="cli-paginate-test-$(date +%s)"
kubectl create namespace "$NAMESPACE" >/dev/null
trap 'kubectl delete namespace "$NAMESPACE" --wait=false >/dev/null 2>&1 || true' EXIT

for i in $(seq 1 25); do
  cat <<EOF | kubectl apply -f - >/dev/null
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r-${i}
  namespace: ${NAMESPACE}
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /r${i}
EOF
done

# Wait for controller to reconcile
sleep 5

OUT=$("$CLI" --server "$ADMIN_URL" get httproute -n "$NAMESPACE" --limit 10 --all 2>&1)
NAMES=$(echo "$OUT" | grep -oE 'r-[0-9]+' | sort -u)
# Keep the zero count as assertion input instead of letting grep's status 1
# terminate the script before it can print the diagnostic below.
COUNT=$(echo "$NAMES" | edgion_count_nonempty_lines)

if [[ "$COUNT" -ne 25 ]]; then
    echo "FAIL: expected 25 unique names, got $COUNT"
    echo "$OUT"
    exit 1
fi

echo "PASS: cli-get-paginate.sh"
