#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMANCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GATEWAY_CLASS="${GATEWAY_CLASS:-edgion}"
CLEANUP="${CLEANUP:-false}"
DEBUG="${DEBUG:-true}"
RUN_TEST="${RUN_TEST:-}"
REPORT_OUTPUT="${REPORT_OUTPUT:-$CONFORMANCE_ROOT/conformance-report.yaml}"
TIMEOUT="${TIMEOUT:-30m}"

# Default: Core features only.
# Expand this list as more features are implemented.
SUPPORTED_FEATURES="${SUPPORTED_FEATURES:-Gateway,HTTPRoute,ReferenceGrant}"

show_help() {
  cat <<USAGE
Usage: $0 [options]

Run Gateway API conformance tests against an Edgion cluster.

Options:
  --gateway-class <name>       GatewayClass name (default: edgion)
  --supported-features <list>  Comma-separated feature list (default: Gateway,HTTPRoute,ReferenceGrant)
  --all-features               Enable all supported features for Extended run
  --run-test <name>            Run a specific test by name
  --cleanup                    Clean up base resources after tests
  --no-debug                   Disable debug output
  --timeout <duration>         Test timeout (default: 30m)
  --report <path>              Conformance report output path
  --port-forward               Auto-start kubectl port-forward and route traffic
                               through localhost (for clusters with unreachable pod IPs)
  -h, --help                   Show this help

Feature Presets:
  --core       Core only: Gateway,HTTPRoute,ReferenceGrant
  --extended   Core + all implemented Extended features

Environment Variables (port-forward proxy):
  GATEWAY_PROXY_HOST           Proxy host for gateway traffic (enables proxy mode)
  GATEWAY_PROXY_HTTP_PORT      Local HTTP port (default: 8080)
  GATEWAY_PROXY_HTTPS_PORT     Local HTTPS port (default: 8443)

Examples:
  $0                                    # Core features
  $0 --extended                         # Core + Extended
  $0 --run-test HTTPRouteSimpleSameNamespace  # Single test
  $0 --port-forward                     # Auto port-forward mode
USAGE
}

PORT_FORWARD="${PORT_FORWARD:-false}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --gateway-class)       GATEWAY_CLASS="$2";       shift 2 ;;
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
HTTPRouteResponseHeaderModification,HTTPRouteBackendRequestHeaderModification,\
HTTPRoutePortRedirect,HTTPRouteSchemeRedirect,HTTPRoutePathRedirect,\
HTTPRouteRequestTimeout,HTTPRouteBackendTimeout,\
HTTPRoute303RedirectStatusCode,HTTPRoute307RedirectStatusCode,HTTPRoute308RedirectStatusCode,\
GRPCRoute,TCPRoute,GatewayPort8080"
      shift ;;
    --all-features)
      SUPPORTED_FEATURES=""
      shift ;;
    -h|--help) show_help; exit 0 ;;
    *) echo "Unknown option: $1"; show_help; exit 1 ;;
  esac
done

echo "=== Gateway API Conformance Tests ==="
echo "  GatewayClass      : ${GATEWAY_CLASS}"
echo "  Supported features: ${SUPPORTED_FEATURES:-<all>}"
echo "  Cleanup            : ${CLEANUP}"
echo "  Debug              : ${DEBUG}"
echo "  Timeout            : ${TIMEOUT}"
if [[ -n "${RUN_TEST}" ]]; then
  echo "  Run test           : ${RUN_TEST}"
fi
echo ""

cd "$CONFORMANCE_ROOT"

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
)

if [[ -n "${SUPPORTED_FEATURES}" ]]; then
  ARGS+=("-supported-features=${SUPPORTED_FEATURES}")
else
  ARGS+=("-all-features")
fi

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
  export GATEWAY_PROXY_HOST="${GATEWAY_PROXY_HOST:-127.0.0.1}"
  export GATEWAY_PROXY_HTTP_PORT="${GATEWAY_PROXY_HTTP_PORT:-8080}"
  export GATEWAY_PROXY_HTTPS_PORT="${GATEWAY_PROXY_HTTPS_PORT:-8443}"

  GW_NS="edgion-system"
  GW_POD=$(kubectl get pods -n "${GW_NS}" -l app=edgion-gateway -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
  if [[ -z "${GW_POD}" ]]; then
    echo "ERROR: No edgion-gateway pod found in ${GW_NS}" >&2
    exit 1
  fi

  echo "Starting port-forward: ${GW_POD} ${GATEWAY_PROXY_HTTP_PORT}:80 ${GATEWAY_PROXY_HTTPS_PORT}:443"
  kubectl port-forward "pod/${GW_POD}" -n "${GW_NS}" \
    "${GATEWAY_PROXY_HTTP_PORT}:80" "${GATEWAY_PROXY_HTTPS_PORT}:443" \
    --address="${GATEWAY_PROXY_HOST}" >/dev/null 2>&1 &
  PF_PID=$!
  sleep 2
  if ! kill -0 "${PF_PID}" 2>/dev/null; then
    echo "ERROR: port-forward failed to start" >&2
    exit 1
  fi
  trap cleanup_pf EXIT
fi

echo "Running: go test ./... -run TestConformance -v -timeout ${TIMEOUT} -args ${ARGS[*]}"
echo ""

go test ./... \
  -run TestConformance \
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
