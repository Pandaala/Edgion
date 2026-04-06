#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMANCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT="$(cd "$CONFORMANCE_ROOT/../.." && pwd)"

TAG="conformance-dev"
CONTROLLER_IMAGE="pandaala/edgion-controller:${TAG}"
GATEWAY_IMAGE="pandaala/edgion-gateway:${TAG}"
NS="edgion-system"
PROFILE="${PROFILE:-debug}"

usage() {
  cat <<EOF
Gateway API Conformance - OrbStack Dev Tool

Usage: $0 <command> [args...]
       PROFILE=release $0 <command>   # use release build (default: debug)

Commands:
  build [c|g|all]       Build Docker image(s) (default: all)
  deploy [c|g|all]      Update Deployment image + rollout (default: all)
  rebuild [c|g|all]     build + deploy in one step (default: all)
  hotpatch [c|g]        Extract binary from image, cp to Pod, kill process
  localpatch [c|g|all]  Fast: cargo zigbuild locally → kubectl cp → pkill (no Docker build)
  test [args...]        Run conformance test (auto port-forward)
  status                Show Pod/GatewayClass/Gateway status
  logs [c|g]            Tail stdout logs (process start/crash only)
  file-log [c|g]        Tail file log from pod (--tail N, --grep PATTERN, -f to follow)
  access-log            View access log from gateway pod (--tail N, --grep PATTERN, -f to follow)
  tls-log               View TLS access log from gateway pod (--tail N, --grep PATTERN)
  find-sv <sv>          Search sv value across gateway + controller logs + access log
  route-status [type]   Show route conditions (http|grpc|tls|all, default: all)
  dump-debug            Snapshot full debug state: pods + routes + last logs
  restart [c|g|all]     Restart deployment(s)
  shell [c|g]           Exec into controller or gateway pod

Examples:
  $0 rebuild                                    # Rebuild all + deploy
  $0 rebuild c                                  # Rebuild controller only
  $0 test --run-test HTTPRouteSimpleSameNamespace
  $0 test --core                                # All core tests
  $0 file-log g --tail 50                       # Last 50 lines of gateway file log
  $0 access-log --grep foo.com                  # Access log entries for foo.com
  $0 find-sv 42                                 # Trace sv=42 across all logs
  $0 route-status grpc                          # All GRPCRoute conditions
  $0 dump-debug                                 # Full debug snapshot
  $0 logs c                                     # Controller stdout
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
  echo "  Profile: ${PROFILE}"

  if [[ "$target" == "c" || "$target" == "all" ]]; then
    echo "Building controller image..."
    docker build \
      --build-arg BINARY=edgion-controller \
      --build-arg BUILD_TYPE=bin \
      --build-arg PROFILE="${PROFILE}" \
      -t "${CONTROLLER_IMAGE}" \
      -f docker/Dockerfile .
    echo "Controller image built: ${CONTROLLER_IMAGE}"
  fi

  if [[ "$target" == "g" || "$target" == "all" ]]; then
    echo "Building gateway image..."
    docker build \
      --build-arg BINARY=edgion-gateway \
      --build-arg BUILD_TYPE=bin \
      --build-arg PROFILE="${PROFILE}" \
      -t "${GATEWAY_IMAGE}" \
      -f docker/Dockerfile .
    echo "Gateway image built: ${GATEWAY_IMAGE}"
  fi
}

cmd_deploy() {
  local target="${1:-all}"
  local backtrace_val
  backtrace_val=$([ "${PROFILE}" = "release" ] && echo "0" || echo "full")

  if [[ "$target" == "c" || "$target" == "all" ]]; then
    echo "Deploying controller (RUST_BACKTRACE=${backtrace_val})..."
    kubectl -n "$NS" set image deployment/edgion-controller \
      edgion-controller="${CONTROLLER_IMAGE}"
    kubectl -n "$NS" set env deployment/edgion-controller \
      RUST_BACKTRACE="${backtrace_val}"
    kubectl rollout restart deployment/edgion-controller -n "$NS"
    kubectl rollout status deployment/edgion-controller -n "$NS" --timeout=120s
  fi

  if [[ "$target" == "g" || "$target" == "all" ]]; then
    echo "Deploying gateway (RUST_BACKTRACE=${backtrace_val})..."
    kubectl -n "$NS" set image deployment/edgion-gateway \
      edgion-gateway="${GATEWAY_IMAGE}"
    kubectl -n "$NS" set env deployment/edgion-gateway \
      RUST_BACKTRACE="${backtrace_val}"
    kubectl rollout restart deployment/edgion-gateway -n "$NS"
    kubectl rollout status deployment/edgion-gateway -n "$NS" --timeout=120s
  fi
}

cmd_localpatch() {
  # Fast iteration: cargo zigbuild (local, cross-compile to Linux arm64) → kubectl cp → pkill.
  # No Docker build — binary goes into /usr/local/edgion/ (edgion-owned, writable).
  # The restart-loop entrypoint picks up the new binary automatically after pkill.
  #
  # One-time setup (if not done):
  #   brew install zig
  #   cargo install cargo-zigbuild
  #   rustup target add aarch64-unknown-linux-gnu
  local target="${1:-all}"
  local cargo_profile_flag
  cargo_profile_flag=$([ "${PROFILE}" = "release" ] && echo "--release" || echo "")
  local out_dir
  out_dir=$([ "${PROFILE}" = "release" ] && echo "release" || echo "debug")

  cd "$PROJECT_ROOT"

  # Check prerequisites
  command -v cargo-zigbuild >/dev/null 2>&1 || { echo "ERROR: cargo-zigbuild not found. Run: brew install zig && cargo install cargo-zigbuild && rustup target add aarch64-unknown-linux-gnu"; exit 1; }

  if [[ "$target" == "c" || "$target" == "all" ]]; then
    echo "Building edgion-controller (local, ${PROFILE})..."
    cargo zigbuild ${cargo_profile_flag} --target aarch64-unknown-linux-gnu --bin edgion-controller

    local pod
    pod=$(get_pod controller)
    [[ -z "$pod" ]] && { echo "ERROR: No controller pod found"; exit 1; }

    echo "Copying to pod ${pod}..."
    kubectl cp "target/aarch64-unknown-linux-gnu/${out_dir}/edgion-controller" \
      "${NS}/${pod}:/usr/local/edgion/edgion-controller"

    echo "Restarting controller process..."
    kubectl exec -n "$NS" "$pod" -- pkill -f "edgion-controller" || true
    sleep 2
    echo "Done. Controller restarted with new binary."
  fi

  if [[ "$target" == "g" || "$target" == "all" ]]; then
    echo "Building edgion-gateway (local, ${PROFILE})..."
    cargo zigbuild ${cargo_profile_flag} --target aarch64-unknown-linux-gnu --bin edgion-gateway

    local pod
    pod=$(get_pod gateway)
    [[ -z "$pod" ]] && { echo "ERROR: No gateway pod found"; exit 1; }

    echo "Copying to pod ${pod}..."
    kubectl cp "target/aarch64-unknown-linux-gnu/${out_dir}/edgion-gateway" \
      "${NS}/${pod}:/usr/local/edgion/edgion-gateway"

    echo "Restarting gateway process..."
    kubectl exec -n "$NS" "$pod" -- pkill -f "edgion-gateway" || true
    sleep 2
    echo "Done. Gateway restarted with new binary."
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

# ─── Log file helpers ────────────────────────────────────────────────────────
#
# All meaningful Edgion logs (access log, system log) are written to files
# inside the pod — NOT to stdout. kubectl logs only sees process stdout which
# contains very little. Use these commands to read the actual log files.
#
# Log file paths inside pods:
#   gateway system log : /usr/local/edgion/logs/edgion-gateway.<YYYY-MM-DD>
#   controller sys log : /usr/local/edgion/logs/edgion-controller.<YYYY-MM-DD>
#   access log         : /usr/local/edgion/logs/edgion_access.log
#   TLS access log     : /usr/local/edgion/logs/tls_access.log
# ─────────────────────────────────────────────────────────────────────────────

cmd_file_log() {
  local target="${1:-g}"; shift || true
  local tail_n=100 grep_pat="" follow=false
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --tail)  tail_n="$2"; shift 2 ;;
      --grep)  grep_pat="$2"; shift 2 ;;
      -f|--follow) follow=true; shift ;;
      *) echo "Unknown option: $1"; exit 1 ;;
    esac
  done

  local app pod log_path
  [[ "$target" == "c" ]] && app="controller" || app="gateway"
  pod=$(get_pod "$app")
  [[ -z "$pod" ]] && { echo "ERROR: Pod not found for $app"; exit 1; }
  log_path="/usr/local/edgion/logs/edgion-${app}.$(date +%Y-%m-%d)"

  if [[ -n "$grep_pat" ]]; then
    kubectl exec -n "$NS" "$pod" -- grep -i "$grep_pat" "$log_path" 2>/dev/null | tail -n "$tail_n" \
      || echo "(no matches for: $grep_pat)"
  elif [[ "$follow" == true ]]; then
    kubectl exec -n "$NS" "$pod" -- tail -n "$tail_n" -f "$log_path"
  else
    kubectl exec -n "$NS" "$pod" -- tail -n "$tail_n" "$log_path"
  fi
}

cmd_access_log() {
  local tail_n=50 grep_pat="" follow=false
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --tail)  tail_n="$2"; shift 2 ;;
      --grep)  grep_pat="$2"; shift 2 ;;
      -f|--follow) follow=true; shift ;;
      *) echo "Unknown option: $1"; exit 1 ;;
    esac
  done

  local pod log_path
  pod=$(get_pod gateway)
  [[ -z "$pod" ]] && { echo "ERROR: Gateway pod not found"; exit 1; }
  log_path="/usr/local/edgion/logs/edgion_access.log"

  if [[ -n "$grep_pat" ]]; then
    kubectl exec -n "$NS" "$pod" -- grep "$grep_pat" "$log_path" 2>/dev/null \
      | tail -n "$tail_n" | jq -c . 2>/dev/null || true
  elif [[ "$follow" == true ]]; then
    kubectl exec -n "$NS" "$pod" -- tail -n "$tail_n" -f "$log_path" | jq -c . 2>/dev/null || true
  else
    kubectl exec -n "$NS" "$pod" -- tail -n "$tail_n" "$log_path" | jq -c . 2>/dev/null || true
  fi
}

cmd_tls_log() {
  local tail_n=50 grep_pat=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --tail) tail_n="$2"; shift 2 ;;
      --grep) grep_pat="$2"; shift 2 ;;
      *) echo "Unknown option: $1"; exit 1 ;;
    esac
  done

  local pod log_path
  pod=$(get_pod gateway)
  [[ -z "$pod" ]] && { echo "ERROR: Gateway pod not found"; exit 1; }
  log_path="/usr/local/edgion/logs/tls_access.log"

  if [[ -n "$grep_pat" ]]; then
    kubectl exec -n "$NS" "$pod" -- grep "$grep_pat" "$log_path" 2>/dev/null \
      | tail -n "$tail_n" | jq -c . 2>/dev/null || true
  else
    kubectl exec -n "$NS" "$pod" -- tail -n "$tail_n" "$log_path" | jq -c . 2>/dev/null || true
  fi
}

# Search sv value across all log files.
# sv (sync_version) is the key that links controller config sync events
# to the access log and gateway log — use it to trace a request failure
# back to the config change that caused it.
cmd_find_sv() {
  local sv="${1:-}"
  [[ -z "$sv" ]] && { echo "Usage: $0 find-sv <sv_value>"; exit 1; }

  local gw_pod ctl_pod gw_log ctl_log today
  gw_pod=$(get_pod gateway)
  ctl_pod=$(get_pod controller)
  today=$(date +%Y-%m-%d)
  gw_log="/usr/local/edgion/logs/edgion-gateway.${today}"
  ctl_log="/usr/local/edgion/logs/edgion-controller.${today}"

  echo "=== Gateway log (sv=${sv}) ==="
  kubectl exec -n "$NS" "$gw_pod" -- grep "\"sv\":${sv}\b\|sv=${sv}\b" "$gw_log" 2>/dev/null \
    || echo "  (no matches)"

  echo ""
  echo "=== Controller log (sv=${sv}) ==="
  kubectl exec -n "$NS" "$ctl_pod" -- grep "\"sv\":${sv}\b\|sv=${sv}\b" "$ctl_log" 2>/dev/null \
    || echo "  (no matches)"

  echo ""
  echo "=== Access log (sv=${sv}) ==="
  kubectl exec -n "$NS" "$gw_pod" -- grep "\"sv\":${sv}" \
    /usr/local/edgion/logs/edgion_access.log 2>/dev/null \
    | tail -20 | jq -c . 2>/dev/null || echo "  (no matches)"
}

# Show K8s route status conditions — useful to confirm whether routes have
# been accepted and resolved before chasing data-plane logs.
cmd_route_status() {
  local rtype="${1:-all}"
  local ns_flag="-A"

  print_route_conditions() {
    local kind="$1"
    echo "=== ${kind} ==="
    kubectl get "${kind}" ${ns_flag} 2>/dev/null \
      -o custom-columns="NS:.metadata.namespace,NAME:.metadata.name,ACCEPTED:.status.conditions[?(@.type=='Accepted')].status,RESOLVEDREFS:.status.conditions[?(@.type=='ResolvedRefs')].status" \
      2>/dev/null || echo "  (CRD not available or no resources)"
    echo ""
  }

  case "$rtype" in
    http)  print_route_conditions httproute ;;
    grpc)  print_route_conditions grpcroute ;;
    tls)   print_route_conditions tlsroute ;;
    all|*) print_route_conditions httproute
           print_route_conditions grpcroute
           print_route_conditions tlsroute ;;
  esac
}

# Collect all debug state into a single snapshot — use this when a
# conformance test fails and you need to understand the full picture.
cmd_dump_debug() {
  local gw_pod ctl_pod today
  gw_pod=$(get_pod gateway)
  ctl_pod=$(get_pod controller)
  today=$(date +%Y-%m-%d)

  echo "=== $(date) — Edgion Conformance Debug Dump ==="
  echo ""

  echo "─── Pods ────────────────────────────────────────────"
  kubectl get pods -n "$NS" -o wide
  echo ""

  echo "─── GatewayClass ────────────────────────────────────"
  kubectl get gatewayclass edgion -o jsonpath='{.status.conditions}' 2>/dev/null | jq . || true
  echo ""

  echo "─── Conformance Gateways ────────────────────────────"
  kubectl get gateway -A -o wide 2>/dev/null | head -20 || echo "  (none)"
  echo ""

  echo "─── Route status ────────────────────────────────────"
  cmd_route_status all
  echo ""

  echo "─── Gateway file log (last 60) ──────────────────────"
  [[ -n "$gw_pod" ]] && \
    kubectl exec -n "$NS" "$gw_pod" -- tail -60 \
      "/usr/local/edgion/logs/edgion-gateway.${today}" 2>/dev/null \
    || echo "  (not available)"
  echo ""

  echo "─── Controller file log (last 30) ───────────────────"
  [[ -n "$ctl_pod" ]] && \
    kubectl exec -n "$NS" "$ctl_pod" -- tail -30 \
      "/usr/local/edgion/logs/edgion-controller.${today}" 2>/dev/null \
    || echo "  (not available)"
  echo ""

  echo "─── Access log (last 20) ────────────────────────────"
  [[ -n "$gw_pod" ]] && \
    kubectl exec -n "$NS" "$gw_pod" -- tail -20 \
      /usr/local/edgion/logs/edgion_access.log 2>/dev/null \
    | jq -c . 2>/dev/null \
    || echo "  (not available)"
}
# ─────────────────────────────────────────────────────────────────────────────

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
  build)        shift; cmd_build "${1:-all}" ;;
  deploy)       shift; cmd_deploy "${1:-all}" ;;
  rebuild)      shift; cmd_rebuild "${1:-all}" ;;
  localpatch)   shift; cmd_localpatch "${1:-all}" ;;
  hotpatch)     shift; cmd_hotpatch "${1:-}" ;;
  test)         shift; cmd_test "$@" ;;
  status)       cmd_status ;;
  logs)         shift; cmd_logs "${1:-c}" ;;
  file-log)     shift; cmd_file_log "${1:-g}" "${@:2}" ;;
  access-log)   shift; cmd_access_log "$@" ;;
  tls-log)      shift; cmd_tls_log "$@" ;;
  find-sv)      shift; cmd_find_sv "${1:-}" ;;
  route-status) shift; cmd_route_status "${1:-all}" ;;
  dump-debug)   cmd_dump_debug ;;
  restart)      shift; cmd_restart "${1:-all}" ;;
  shell)        shift; cmd_shell "${1:-c}" ;;
  -h|--help|help|"") usage ;;
  *) echo "Unknown command: $1"; usage; exit 1 ;;
esac
