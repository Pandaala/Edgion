#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMANCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT="$(cd "$CONFORMANCE_ROOT/../.." && pwd)"

TAG="conformance-dev"
CONTROLLER_IMAGE="pandaala/edgion-controller:${TAG}"
GATEWAY_IMAGE="pandaala/edgion-gateway:${TAG}"
NS="edgion-system"

usage() {
  cat <<EOF
Gateway API Conformance - OrbStack Dev Tool

Usage: $0 <command> [args...]

Commands:
  build [c|g|all]       Build Docker image(s) (default: all)
  deploy [c|g|all]      Update Deployment image + rollout (default: all)
  rebuild [c|g|all]     build + deploy in one step (default: all)
  hotpatch [c|g]        Extract binary from image, cp to Pod, kill process
  test [args...]        Run conformance test (auto port-forward)
  status                Show Pod/GatewayClass/Gateway status
  logs [c|g]            Tail controller or gateway logs
  restart [c|g|all]     Restart deployment(s)
  shell [c|g]           Exec into controller or gateway pod

Examples:
  $0 rebuild                                    # Rebuild all + deploy
  $0 rebuild c                                  # Rebuild controller only
  $0 test --run-test HTTPRouteSimpleSameNamespace
  $0 test --core                                # All core tests
  $0 logs c                                     # Controller logs
  $0 status                                     # Cluster status
EOF
}

get_pod() {
  local app="$1"
  kubectl get pods -n "$NS" -l "app=edgion-${app}" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null
}

cmd_build() {
  local target="${1:-all}"
  cd "$PROJECT_ROOT"

  if [[ "$target" == "c" || "$target" == "all" ]]; then
    echo "Building controller image..."
    docker build \
      --build-arg BINARY=edgion-controller \
      --build-arg BUILD_TYPE=bin \
      -t "${CONTROLLER_IMAGE}" \
      -f docker/Dockerfile .
    echo "Controller image built: ${CONTROLLER_IMAGE}"
  fi

  if [[ "$target" == "g" || "$target" == "all" ]]; then
    echo "Building gateway image..."
    docker build \
      --build-arg BINARY=edgion-gateway \
      --build-arg BUILD_TYPE=bin \
      -t "${GATEWAY_IMAGE}" \
      -f docker/Dockerfile .
    echo "Gateway image built: ${GATEWAY_IMAGE}"
  fi
}

cmd_deploy() {
  local target="${1:-all}"

  if [[ "$target" == "c" || "$target" == "all" ]]; then
    echo "Deploying controller..."
    kubectl -n "$NS" set image deployment/edgion-controller \
      edgion-controller="${CONTROLLER_IMAGE}"
    kubectl rollout restart deployment/edgion-controller -n "$NS"
    kubectl rollout status deployment/edgion-controller -n "$NS" --timeout=120s
  fi

  if [[ "$target" == "g" || "$target" == "all" ]]; then
    echo "Deploying gateway..."
    kubectl -n "$NS" set image deployment/edgion-gateway \
      edgion-gateway="${GATEWAY_IMAGE}"
    kubectl rollout restart deployment/edgion-gateway -n "$NS"
    kubectl rollout status deployment/edgion-gateway -n "$NS" --timeout=120s
  fi
}

cmd_rebuild() {
  local target="${1:-all}"
  cmd_build "$target"
  cmd_deploy "$target"
}

cmd_hotpatch() {
  # Fastest iteration: extract binary from Docker image, cp to running Pod, kill process.
  # The shell restart loop in the Deployment automatically picks up the new binary.
  local target="${1:-}"
  [[ -z "$target" ]] && { echo "Usage: $0 hotpatch [c|g]"; exit 1; }

  cd "$PROJECT_ROOT"

  if [[ "$target" == "c" ]]; then
    local pod
    pod=$(get_pod controller)
    [[ -z "$pod" ]] && { echo "ERROR: No controller pod found"; exit 1; }

    echo "Building controller image..."
    docker build \
      --build-arg BINARY=edgion-controller \
      --build-arg BUILD_TYPE=bin \
      -t "${CONTROLLER_IMAGE}" \
      -f docker/Dockerfile .

    echo "Extracting binary from image..."
    local cid
    cid=$(docker create "${CONTROLLER_IMAGE}")
    docker cp "${cid}:/usr/local/bin/edgion-controller" "/tmp/edgion-controller"
    docker rm "${cid}" >/dev/null

    echo "Copying to pod ${pod}..."
    kubectl cp "/tmp/edgion-controller" "${NS}/${pod}:/usr/local/edgion/edgion-controller"

    echo "Killing controller process (will auto-restart)..."
    kubectl exec -n "$NS" "$pod" -- pkill -f "edgion-controller" || true
    sleep 3
    echo "Done. Controller restarted with new binary."

  elif [[ "$target" == "g" ]]; then
    local pod
    pod=$(get_pod gateway)
    [[ -z "$pod" ]] && { echo "ERROR: No gateway pod found"; exit 1; }

    echo "Building gateway image..."
    docker build \
      --build-arg BINARY=edgion-gateway \
      --build-arg BUILD_TYPE=bin \
      -t "${GATEWAY_IMAGE}" \
      -f docker/Dockerfile .

    echo "Extracting binary from image..."
    local cid
    cid=$(docker create "${GATEWAY_IMAGE}")
    docker cp "${cid}:/usr/local/bin/edgion-gateway" "/tmp/edgion-gateway"
    docker rm "${cid}" >/dev/null

    echo "Copying to pod ${pod}..."
    kubectl cp "/tmp/edgion-gateway" "${NS}/${pod}:/usr/local/edgion/edgion-gateway"

    echo "Killing gateway process (will auto-restart)..."
    kubectl exec -n "$NS" "$pod" -- pkill -f "edgion-gateway" || true
    sleep 3
    echo "Done. Gateway restarted with new binary."
  else
    echo "Usage: $0 hotpatch [c|g]"; exit 1
  fi
}

cmd_test() {
  echo "Running conformance test with port-forward..."
  "$SCRIPT_DIR/run.sh" --port-forward "$@"
}

cmd_status() {
  echo "=== Pods ==="
  kubectl get pods -n "$NS" -o wide
  echo ""
  echo "=== GatewayClass ==="
  kubectl get gatewayclass edgion -o json 2>/dev/null \
    | python3 -c "import sys,json; cs=json.load(sys.stdin).get('status',{}).get('conditions',[]); [print(f'  {c[\"type\"]}={c[\"status\"]} ({c.get(\"reason\",\"-\")})') for c in cs]" \
    2>/dev/null || echo "  Not found or no status"
  echo ""
  echo "=== Conformance Gateways ==="
  for gw in same-namespace all-namespaces backend-namespaces same-namespace-with-https-listener; do
    ADDR=$(kubectl -n gateway-conformance-infra get gateway "$gw" -o jsonpath='{.status.addresses[0].value}' 2>/dev/null || true)
    STATUS=$(kubectl -n gateway-conformance-infra get gateway "$gw" -o jsonpath='{.status.conditions[?(@.type=="Accepted")].status}' 2>/dev/null || true)
    printf "  %-45s Accepted=%-5s Address=%s\n" "$gw" "${STATUS:-?}" "${ADDR:-none}"
  done
}

cmd_logs() {
  local target="${1:-c}"
  if [[ "$target" == "c" ]]; then
    kubectl logs -n "$NS" -l app=edgion-controller -f --tail=100
  elif [[ "$target" == "g" ]]; then
    kubectl logs -n "$NS" -l app=edgion-gateway -f --tail=100
  else
    echo "Usage: $0 logs [c|g]"
  fi
}

cmd_restart() {
  local target="${1:-all}"
  if [[ "$target" == "c" || "$target" == "all" ]]; then
    kubectl rollout restart deployment/edgion-controller -n "$NS"
    kubectl rollout status deployment/edgion-controller -n "$NS" --timeout=120s
  fi
  if [[ "$target" == "g" || "$target" == "all" ]]; then
    kubectl rollout restart deployment/edgion-gateway -n "$NS"
    kubectl rollout status deployment/edgion-gateway -n "$NS" --timeout=120s
  fi
}

cmd_shell() {
  local target="${1:-c}"
  local pod
  if [[ "$target" == "c" ]]; then
    pod=$(get_pod controller)
  elif [[ "$target" == "g" ]]; then
    pod=$(get_pod gateway)
  else
    echo "Usage: $0 shell [c|g]"; exit 1
  fi
  [[ -z "$pod" ]] && { echo "ERROR: Pod not found"; exit 1; }
  kubectl exec -it -n "$NS" "$pod" -- /bin/sh
}

# Main dispatch
case "${1:-}" in
  build)    shift; cmd_build "${1:-all}" ;;
  deploy)   shift; cmd_deploy "${1:-all}" ;;
  rebuild)  shift; cmd_rebuild "${1:-all}" ;;
  hotpatch) shift; cmd_hotpatch "${1:-}" ;;
  test)     shift; cmd_test "$@" ;;
  status)   cmd_status ;;
  logs)     shift; cmd_logs "${1:-c}" ;;
  restart)  shift; cmd_restart "${1:-all}" ;;
  shell)    shift; cmd_shell "${1:-c}" ;;
  -h|--help|help|"") usage ;;
  *) echo "Unknown command: $1"; usage; exit 1 ;;
esac
