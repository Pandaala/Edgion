#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMANCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GATEWAY_CLASS="${GATEWAY_CLASS:-edgion}"
CLEANUP="${CLEANUP:-false}"
DEBUG="${DEBUG:-true}"
RUN_TEST="${RUN_TEST:-}"
REPORT_OUTPUT="${REPORT_OUTPUT:-$CONFORMANCE_ROOT/conformance-report.yaml}"
CONFORMANCE_PROFILES="${CONFORMANCE_PROFILES:-GATEWAY-HTTP,GATEWAY-GRPC,GATEWAY-TLS}"
TIMEOUT="${TIMEOUT:-30m}"

# Default: empty — features are declared in conformance_test.go opts.SupportedFeatures.
# Override with --supported-features, --core, --extended, or --all-features flags.
SUPPORTED_FEATURES="${SUPPORTED_FEATURES:-}"

show_help() {
  cat <<USAGE
Usage: $0 [options]

Run Gateway API conformance tests against an Edgion cluster.

Options:
  --gateway-class <name>        GatewayClass name (default: edgion)
  --supported-features <list>   Comma-separated feature list (default: Gateway,HTTPRoute,ReferenceGrant)
  --all-features                Enable all supported features for Extended run
  --run-test <name>             Run a specific test by name
  --cleanup                     Clean up base resources after tests
  --no-debug                    Disable debug output
  --timeout <duration>          Test timeout (default: 30m)
  --report <path>               Conformance report output path
  --conformance-profiles <list> Comma-separated conformance profile list
                                (default: GATEWAY-HTTP,GATEWAY-GRPC,GATEWAY-TLS; options: GATEWAY-HTTP,GATEWAY-GRPC,GATEWAY-TLS)
  --port-forward                Auto-start kubectl port-forward and route traffic
                                through localhost (for clusters with unreachable pod IPs)
  -h, --help                    Show this help

Feature Presets:
  --core       Core only: Gateway,HTTPRoute,ReferenceGrant
  --extended   Core + all implemented Extended features

Transport Access Layer (TAL) — Environment Variables:
  GATEWAY_PROXY_HOST            Proxy host for gateway traffic (enables TAL port-forward mode)
  GATEWAY_PROXY_PORT_MAP        Port mapping table: "gwPort:localPort,..." (preferred)
                                Example: "80:18080,443:18443,8443:28443"
  GATEWAY_PROXY_HTTP_PORT       Local HTTP port (default: 18080, used when PORT_MAP not set)
  GATEWAY_PROXY_HTTPS_PORT      Local HTTPS port (default: 18443, used when PORT_MAP not set)
  GATEWAY_PROXY_HTTPS_8443_PORT Local HTTPS-8443 port (default: 28443, used when PORT_MAP not set)
  GATEWAY_PROXY_ENABLE_GRPC     Enable TAL gRPC client as well (default: false)
  TAL_DEBUG                     Set to "true" to log TAL address rewrites

Examples:
  $0                                         # Core features, direct mode
  $0 --extended                              # Core + Extended
  $0 --run-test HTTPRouteSimpleSameNamespace # Single test
  $0 --port-forward                          # Auto port-forward mode
USAGE
}

# ─── OrbStack K8s routing check ──────────────────────────────────────────────
#
# On OrbStack, K8s pod/service CIDRs (192.168.194.x) are routed via the
# OrbStack internal tunnel (utun4). After an OrbStack restart these routes
# can be dropped, causing all ClusterIP traffic to leak to the physical
# router and time out. This function detects that condition and, if OrbStack
# is present, auto-enables TAL proxy mode via the K8s node's OrbStack IP
# (orbstack-k8s.internal) so tests work without a full OrbStack restart.
#
# Detection flow:
#   1. Quick TCP probe to ClusterIP:80                   → reachable? done.
#   2. DNS query to OrbStack resolver (198.18.0.2)
#      for orbstack-k8s.internal                         → resolved? use it.
#   3. TCP probe to <orbstack-k8s-ip>:80                 → reachable? auto-proxy.
#   4. Not reachable at all                              → fatal, ask restart.
check_gateway_routing() {
  # Only relevant when TAL proxy is not already configured by the caller.
  if [[ -n "${GATEWAY_PROXY_HOST:-}" ]]; then
    return
  fi

  local gw_svc_ip
  gw_svc_ip=$(kubectl get svc edgion-gateway -n edgion-system \
    -o jsonpath='{.spec.clusterIP}' 2>/dev/null || true)
  if [[ -z "${gw_svc_ip}" ]]; then
    return  # Service not found; check_gateway_pod_ready will give a clearer error.
  fi

  # Probe ClusterIP:80 with a 2-second timeout.
  if bash -c "echo >/dev/tcp/${gw_svc_ip}/80" 2>/dev/null; then
    return  # Direct ClusterIP access is working fine.
  fi

  echo "⚠  ClusterIP ${gw_svc_ip}:80 is not reachable — checking OrbStack routing..."

  # Ask OrbStack's own DNS resolver for the K8s node address.
  local orb_node_ip
  orb_node_ip=$(dig +short +time=2 +tries=1 @198.18.0.2 orbstack-k8s.internal A \
    2>/dev/null | grep -E '^[0-9]+\.' | head -1 || true)

  if [[ -z "${orb_node_ip}" ]]; then
    echo ""
    echo "ERROR: Gateway ClusterIP ${gw_svc_ip} is unreachable and OrbStack K8s"
    echo "       DNS (orbstack-k8s.internal) did not resolve."
    echo ""
    echo "  • If using OrbStack: restart OrbStack.app to restore K8s routing."
    echo "  • Otherwise:         check cluster networking or use --port-forward."
    exit 1
  fi

  # Verify the node itself is reachable via utun4.
  if ! bash -c "echo >/dev/tcp/${orb_node_ip}/80" 2>/dev/null; then
    echo ""
    echo "ERROR: OrbStack K8s node ${orb_node_ip} is also unreachable on port 80."
    echo "  Restart OrbStack.app to restore K8s networking, then retry."
    exit 1
  fi

  echo "   OrbStack K8s node found at ${orb_node_ip} (orbstack-k8s.internal)."
  echo "   Routes for ${gw_svc_ip%.*}.0/25 appear to be missing from the Mac"
  echo "   routing table — likely caused by an OrbStack restart without a full"
  echo "   OrbStack.app restart."
  echo ""
  echo "   Auto-enabling TAL proxy via ${orb_node_ip} so tests can proceed."
  echo "   (Run 'sudo route -n add -net 192.168.194.0/25 ${orb_node_ip}' and"
  echo "    'sudo route -n add -net 192.168.194.128/25 ${orb_node_ip}' to fix"
  echo "    permanently, or restart OrbStack.app.)"
  echo ""

  export GATEWAY_PROXY_HOST="${orb_node_ip}"
  # When using the OrbStack node IP, traffic reaches gateway ports directly
  # (no port-forward offset), so port map must be direct (80:80, 443:443, …).
  export GATEWAY_PROXY_PORT_MAP="${GATEWAY_PROXY_PORT_MAP:-80:80,443:443,8443:8443}"
  # Enable gRPC proxy as well so gRPC conformance tests also go through TAL.
  export GATEWAY_PROXY_ENABLE_GRPC="${GATEWAY_PROXY_ENABLE_GRPC:-true}"
}
# ─────────────────────────────────────────────────────────────────────────────

# ─── Stale process check ─────────────────────────────────────────────────────
#
# Leftover test binaries and port-forward processes from a previous run can
# occupy ports and interfere with the new run. Detect and report them early.
check_stale_processes() {
  local pids pf_pids
  pids=$(pgrep -f "gateway-api-conformance.test" 2>/dev/null || true)
  pf_pids=$(pgrep -f "kubectl port-forward.*edgion" 2>/dev/null || true)

  if [[ -z "${pids}" && -z "${pf_pids}" ]]; then
    return
  fi

  echo "⚠  Stale processes detected from a previous test run:"
  [[ -n "${pids}" ]]    && echo "   gateway-api-conformance.test  PIDs: ${pids}"
  [[ -n "${pf_pids}" ]] && echo "   kubectl port-forward          PIDs: ${pf_pids}"
  echo ""
  echo "   These may cause port conflicts or test interference."
  echo "   To clean up, run:"
  [[ -n "${pids}" ]]    && echo "     pkill -f 'gateway-api-conformance.test'"
  [[ -n "${pf_pids}" ]] && echo "     pkill -f 'kubectl port-forward.*edgion'"
  echo ""
  echo "ERROR: Please clean up stale processes before retrying."
  exit 1
}
# ─────────────────────────────────────────────────────────────────────────────

# ─── Gateway pod readiness check ─────────────────────────────────────────────
#
# If no edgion-gateway pod is Ready the tests will time out en masse.
# Fail fast with a clear message so the user doesn't wait 30 minutes.
check_gateway_pod_ready() {
  local ready_status
  ready_status=$(kubectl get pods -n edgion-system -l app=edgion-gateway \
    -o jsonpath='{.items[*].status.conditions[?(@.type=="Ready")].status}' \
    2>/dev/null || true)

  if echo "${ready_status}" | grep -qw "True"; then
    return  # At least one pod is Ready.
  fi

  echo ""
  echo "ERROR: No edgion-gateway pod is Ready in edgion-system."
  kubectl get pods -n edgion-system -l app=edgion-gateway 2>/dev/null || true
  echo ""
  echo "  Wait for the gateway pod to become Ready, then retry."
  exit 1
}
# ─────────────────────────────────────────────────────────────────────────────

# ─── GatewayClass accepted check ─────────────────────────────────────────────
#
# If the GatewayClass exists but its Accepted condition is not True, the
# controller is not reconciling — all tests will fail immediately.
check_gatewayclass_accepted() {
  local status
  status=$(kubectl get gatewayclass "${GATEWAY_CLASS}" \
    -o jsonpath='{.status.conditions[?(@.type=="Accepted")].status}' \
    2>/dev/null || true)

  if [[ "${status}" == "True" ]]; then
    return
  fi

  echo ""
  if [[ -z "${status}" ]]; then
    echo "ERROR: GatewayClass '${GATEWAY_CLASS}' not found."
  else
    echo "ERROR: GatewayClass '${GATEWAY_CLASS}' is not Accepted (status: ${status})."
    kubectl get gatewayclass "${GATEWAY_CLASS}" 2>/dev/null || true
  fi
  echo ""
  echo "  Ensure the controller is running and the GatewayClass status is Accepted."
  exit 1
}
# ─────────────────────────────────────────────────────────────────────────────

# ─── kubectl connectivity check ──────────────────────────────────────────────
#
# Fail fast if the cluster is unreachable before attempting any kubectl calls.
check_kubectl_connected() {
  if kubectl cluster-info &>/dev/null; then
    return
  fi
  echo ""
  echo "ERROR: Cannot connect to Kubernetes cluster."
  kubectl cluster-info 2>&1 || true
  echo ""
  echo "  Check your kubeconfig and cluster connectivity, then retry."
  exit 1
}
# ─────────────────────────────────────────────────────────────────────────────

# ─── go toolchain check ───────────────────────────────────────────────────────
check_go_installed() {
  if command -v go &>/dev/null; then
    return
  fi
  echo "ERROR: 'go' is not installed or not in PATH."
  echo "  Install Go from https://go.dev/dl/ and retry."
  exit 1
}
# ─────────────────────────────────────────────────────────────────────────────

PORT_FORWARD="${PORT_FORWARD:-false}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --gateway-class)       GATEWAY_CLASS="$2";       shift 2 ;;
    --conformance-profiles) CONFORMANCE_PROFILES="$2"; shift 2 ;;
    --supported-features)  SUPPORTED_FEATURES="$2";  shift 2 ;;
    --run-test)            RUN_TEST="$2";             shift 2 ;;
    --cleanup)             CLEANUP=true;              shift   ;;
    --no-debug)            DEBUG=false;               shift   ;;
    --timeout)             TIMEOUT="$2";              shift 2 ;;
    --report)              REPORT_OUTPUT="$2";        shift 2 ;;
    --port-forward)        PORT_FORWARD=true;         shift   ;;
    --core)
      SUPPORTED_FEATURES="Gateway,HTTPRoute,ReferenceGrant"
      shift ;;
    --extended)
      SUPPORTED_FEATURES="Gateway,HTTPRoute,ReferenceGrant,\
HTTPRouteQueryParamMatching,HTTPRouteMethodMatching,\
HTTPRouteResponseHeaderModification,HTTPRouteHostRewrite,HTTPRoutePathRewrite,\
HTTPRouteRequestMirror,HTTPRouteRequestTimeout,HTTPRouteBackendTimeout,\
HTTPRouteBackendRequestHeaderModification,\
HTTPRouteBackendProtocolH2C,HTTPRouteBackendProtocolWebSocket,HTTPRouteNamedRouteRule,\
GatewayHTTPListenerIsolation,GatewayHTTPSListenerDetectMisdirectedRequests,\
HTTPRoutePathRedirect,HTTPRoutePortRedirect,HTTPRouteSchemeRedirect,\
HTTPRoute303RedirectStatusCode,HTTPRoute307RedirectStatusCode,HTTPRoute308RedirectStatusCode,\
HTTPRouteRequestPercentageMirror,HTTPRouteRequestMultipleMirrors,\
HTTPRouteDestinationPortMatching,\
TLSRoute,GRPCRoute"
      shift ;;
    --all-features)
      SUPPORTED_FEATURES="__ALL__"
      shift ;;
    -h|--help) show_help; exit 0 ;;
    *) echo "Unknown option: $1"; show_help; exit 1 ;;
  esac
done

echo "=== Gateway API Conformance Tests ==="
echo "  GatewayClass      : ${GATEWAY_CLASS}"
echo "  Conformance profiles: ${CONFORMANCE_PROFILES}"
echo "  Supported features: ${SUPPORTED_FEATURES:-<all>}"
echo "  Cleanup            : ${CLEANUP}"
echo "  Debug              : ${DEBUG}"
echo "  Timeout            : ${TIMEOUT}"
if [[ -n "${RUN_TEST}" ]]; then
  echo "  Run test           : ${RUN_TEST}"
fi
echo ""

check_stale_processes
check_go_installed
check_kubectl_connected
check_gateway_routing
check_gateway_pod_ready
check_gatewayclass_accepted

cd "$CONFORMANCE_ROOT"

# Patch upstream conformance library (Go 1.24+ IsCA fix)
source "$SCRIPT_DIR/patch-upstream.sh"

ARGS=(
  "-gateway-class=${GATEWAY_CLASS}"
  "-cleanup-base-resources=${CLEANUP}"
  "-debug=${DEBUG}"
  "-allow-crds-mismatch"
  "-organization=Edgion"
  "-project=Edgion"
  "-url=https://github.com/edgion/edgion"
  "-version=dev"
  "-contact=@edgion"
  "-report-output=${REPORT_OUTPUT}"
  "-conformance-profiles=${CONFORMANCE_PROFILES}"
)

if [[ "${SUPPORTED_FEATURES}" == "__ALL__" ]]; then
  ARGS+=("-all-features")
elif [[ -n "${SUPPORTED_FEATURES}" ]]; then
  # Explicit override: pass to framework (overrides conformance_test.go opts)
  ARGS+=("-supported-features=${SUPPORTED_FEATURES}")
fi
# Empty SUPPORTED_FEATURES = use features declared in conformance_test.go opts.SupportedFeatures

if [[ -n "${RUN_TEST}" ]]; then
  ARGS+=("-run-test=${RUN_TEST}")
fi

PF_PID=""
cleanup_pf() {
  if [[ -n "${PF_PID}" ]]; then
    echo "Stopping port-forward (PID ${PF_PID})..."
    kill "${PF_PID}" 2>/dev/null || true
    wait "${PF_PID}" 2>/dev/null || true
  fi
}

if [[ "${PORT_FORWARD}" == "true" ]]; then
  # TAL port-forward mode: resolve the three local ports, export them so the TAL
  # resolver inside the test binary picks them up, then start kubectl port-forward
  # binding the same ports.
  #
  # Port resolution order (for each of the three gateway ports):
  #   1. GATEWAY_PROXY_PORT_MAP if set (parsed below)
  #   2. Individual GATEWAY_PROXY_*_PORT env vars
  #   3. Built-in defaults (18080 / 18443 / 28443)
  export GATEWAY_PROXY_HOST="${GATEWAY_PROXY_HOST:-127.0.0.1}"
  # Enable gRPC proxy so gRPC conformance tests also go through port-forward.
  export GATEWAY_PROXY_ENABLE_GRPC="${GATEWAY_PROXY_ENABLE_GRPC:-true}"

  if [[ -n "${GATEWAY_PROXY_PORT_MAP:-}" ]]; then
    # Parse GATEWAY_PROXY_PORT_MAP ("80:18080,443:18443,8443:28443") so we know
    # which local ports kubectl port-forward must bind.
    _pf_http=$(echo "${GATEWAY_PROXY_PORT_MAP}" | tr ',' '\n' | grep '^80:'   | cut -d: -f2)
    _pf_https=$(echo "${GATEWAY_PROXY_PORT_MAP}" | tr ',' '\n' | grep '^443:'  | cut -d: -f2)
    _pf_8443=$(echo "${GATEWAY_PROXY_PORT_MAP}"  | tr ',' '\n' | grep '^8443:' | cut -d: -f2)
    # Fall back to individual vars / defaults if a mapping is absent.
    GATEWAY_PROXY_HTTP_PORT="${_pf_http:-${GATEWAY_PROXY_HTTP_PORT:-18080}}"
    GATEWAY_PROXY_HTTPS_PORT="${_pf_https:-${GATEWAY_PROXY_HTTPS_PORT:-18443}}"
    GATEWAY_PROXY_HTTPS_8443_PORT="${_pf_8443:-${GATEWAY_PROXY_HTTPS_8443_PORT:-28443}}"
  else
    GATEWAY_PROXY_HTTP_PORT="${GATEWAY_PROXY_HTTP_PORT:-18080}"
    GATEWAY_PROXY_HTTPS_PORT="${GATEWAY_PROXY_HTTPS_PORT:-18443}"
    GATEWAY_PROXY_HTTPS_8443_PORT="${GATEWAY_PROXY_HTTPS_8443_PORT:-28443}"
  fi
  export GATEWAY_PROXY_HTTP_PORT GATEWAY_PROXY_HTTPS_PORT GATEWAY_PROXY_HTTPS_8443_PORT

  GW_NS="edgion-system"
  GW_POD=$(kubectl get pods -n "${GW_NS}" -l app=edgion-gateway -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
  if [[ -z "${GW_POD}" ]]; then
    echo "ERROR: No edgion-gateway pod found in ${GW_NS}" >&2
    exit 1
  fi

  echo "TAL: Starting port-forward ${GW_POD} → ${GATEWAY_PROXY_HOST}  80:${GATEWAY_PROXY_HTTP_PORT}  443:${GATEWAY_PROXY_HTTPS_PORT}  8443:${GATEWAY_PROXY_HTTPS_8443_PORT}"
  kubectl port-forward "pod/${GW_POD}" -n "${GW_NS}" \
    "${GATEWAY_PROXY_HTTP_PORT}:80" "${GATEWAY_PROXY_HTTPS_PORT}:443" "${GATEWAY_PROXY_HTTPS_8443_PORT}:8443" \
    --address="${GATEWAY_PROXY_HOST}" >/dev/null 2>&1 &
  PF_PID=$!
  sleep 2
  if ! kill -0 "${PF_PID}" 2>/dev/null; then
    echo "ERROR: port-forward failed to start" >&2
    exit 1
  fi
  trap cleanup_pf EXIT
fi

GO_RUN_FILTER="${GO_RUN_FILTER:-TestConformance}"

echo "Running: go test ./... -run ${GO_RUN_FILTER} -v -timeout ${TIMEOUT} -args ${ARGS[*]}"
echo ""

go test ./... \
  -run "${GO_RUN_FILTER}" \
  -v \
  -timeout "${TIMEOUT}" \
  -count=1 \
  -args "${ARGS[@]}"

RC=$?

echo ""
if [[ "$RC" -eq 0 ]]; then
  echo "=== Conformance Tests PASSED ==="
else
  echo "=== Conformance Tests FAILED (exit code: $RC) ==="
fi

if [[ -f "${REPORT_OUTPUT}" ]]; then
  echo "Report saved to: ${REPORT_OUTPUT}"
fi

exit "$RC"
