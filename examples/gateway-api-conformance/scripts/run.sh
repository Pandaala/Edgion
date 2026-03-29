#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMANCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GATEWAY_CLASS="${GATEWAY_CLASS:-edgion}"
CLEANUP="${CLEANUP:-false}"
DEBUG="${DEBUG:-true}"
RUN_TEST="${RUN_TEST:-}"
REPORT_OUTPUT="${REPORT_OUTPUT:-$CONFORMANCE_ROOT/conformance-report.yaml}"
# ⚠ GATEWAY-GRPC declared but not yet validated against real conformance run.
# GATEWAY-TLS not declared — needs SupportTLSRoute + status condition work first.
CONFORMANCE_PROFILES="${CONFORMANCE_PROFILES:-GATEWAY-HTTP,GATEWAY-GRPC}"
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
                                (default: GATEWAY-HTTP; options: GATEWAY-HTTP,GATEWAY-GRPC,GATEWAY-TLS)
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
HTTPRouteResponseHeaderModification,HTTPRouteBackendRequestHeaderModification,\
HTTPRoutePortRedirect,HTTPRouteSchemeRedirect,HTTPRoutePathRedirect,\
HTTPRouteRequestTimeout,HTTPRouteBackendTimeout,\
HTTPRoute303RedirectStatusCode,HTTPRoute307RedirectStatusCode,HTTPRoute308RedirectStatusCode,\
GRPCRoute,TCPRoute"
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
