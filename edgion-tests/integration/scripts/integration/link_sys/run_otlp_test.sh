#!/usr/bin/env bash
# Standalone-topology exception: log primary/fallback outputs are immutable process
# YAML, so the shared integration Gateway cannot exercise this configuration. This
# runner owns one Controller/Gateway/backend and two uniquely named collectors.
# Its Python wait_phase driver applies live LinkSys/Secret generations and checks
# actual delivery; it never restarts the Gateway between dynamic phases.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../../.." && pwd)"
source "${SCRIPT_DIR}/../../utils/conf_sync_certs.sh"
HELPER="${SCRIPT_DIR}/otlp_test_assertions.py"
DO_BUILD=true
DO_CLEANUP=true
for arg in "$@"; do
    case "$arg" in
        --no-build) DO_BUILD=false ;;
        --no-cleanup|--keep-alive) DO_CLEANUP=false ;;
        -h|--help) echo "Usage: $0 [--no-build] [--no-cleanup|--keep-alive]"; exit 0 ;;
        *) echo "Unknown argument: $arg" >&2; exit 2 ;;
    esac
done
export OTLP_PROJECT_ROOT="$PROJECT_ROOT"
export OTLP_WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/otlp_testing_$(date +%Y%m%d_%H%M%S)_$$"
export OTLP_COMPOSE_PROJECT="edgion-otlp-$(date +%s)-$$"
export EDGION_WORK_DIR="$OTLP_WORK_DIR"
export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
export EDGION_TOKEN="$EDGION_ADMIN_TOKEN"
export EDGION_GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-integration-gateway-admin-token-0123456789}"
mkdir -p "$OTLP_WORK_DIR/logs" "$OTLP_WORK_DIR/pids" "$OTLP_WORK_DIR/config" "$OTLP_WORK_DIR/resources"
CONTROLLER_PID=""
GATEWAY_PID=""
BACKEND_PID=""
cleanup() {
    local result=$?
    trap - EXIT
    if [ "$DO_CLEANUP" = true ]; then
        for pid in "$GATEWAY_PID" "$CONTROLLER_PID" "$BACKEND_PID"; do
            if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
                kill -TERM "$pid" 2>/dev/null || true
                if ! python3 "$HELPER" wait-pid "$pid" 45; then
                    echo "Owned process $pid exceeded cleanup deadline; terminating it" >&2
                    kill -KILL "$pid" 2>/dev/null || true
                fi
                wait "$pid" 2>/dev/null || true
            fi
        done
        if [ -f "$OTLP_WORK_DIR/config/collectors.yaml" ]; then
            docker compose -p "$OTLP_COMPOSE_PROJECT" -f "$OTLP_WORK_DIR/config/collectors.yaml" down --timeout 10 || result=1
        fi
    else
        echo "Owned processes and compose project retained: $OTLP_COMPOSE_PROJECT"
    fi
    echo "OTLP test evidence: $OTLP_WORK_DIR"
    exit "$result"
}
trap cleanup EXIT
if [ "$DO_BUILD" = true ]; then
    (cd "$PROJECT_ROOT" && cargo build -p edgion-controller -p edgion-gateway -p edgion-cli -p edgion-tests \
        --bin edgion-controller --bin edgion-gateway --bin edgion-cli --bin test_server)
fi
for bin in edgion-controller edgion-gateway edgion-cli test_server; do
    test -x "$PROJECT_ROOT/target/debug/$bin" || { echo "Missing binary: $bin" >&2; exit 1; }
done
docker info > /dev/null
python3 "$HELPER" prepare
cp -R "$PROJECT_ROOT/config/crd" "$OTLP_WORK_DIR/config/crd"
conf_sync_distribute_certs "$OTLP_WORK_DIR"
"$PROJECT_ROOT/target/debug/edgion-cli" gen-certs --output file \
    --output-dir "$OTLP_WORK_DIR/certs/otlp" --cert-name backend \
    --server-san otlp-backend.edgion-test.example.com --force
conf_sync_security_block server >> "$OTLP_WORK_DIR/config/controller.yaml"
conf_sync_security_block client >> "$OTLP_WORK_DIR/config/gateway.yaml"
"$PROJECT_ROOT/target/debug/edgion-controller" -c "$OTLP_WORK_DIR/config/controller.yaml" \
    --work-dir "$OTLP_WORK_DIR" --conf-dir "$OTLP_WORK_DIR/resources" --test-mode \
    > "$OTLP_WORK_DIR/logs/controller.log" 2>&1 &
CONTROLLER_PID=$!
echo "$CONTROLLER_PID" > "$OTLP_WORK_DIR/pids/controller.pid"
python3 "$HELPER" controller-ready
python3 "$HELPER" apply-initial
"$PROJECT_ROOT/target/debug/test_server" --http-ports "${OTLP_BACKEND_PORT:-33001}" \
    --grpc-ports "" --websocket-port 0 --tcp-port 0 --udp-port 0 \
    --https-backend-port "${OTLP_TLS_BACKEND_PORT:-33002}" \
    --cert-file "$OTLP_WORK_DIR/certs/otlp/backend/server.crt" \
    --key-file "$OTLP_WORK_DIR/certs/otlp/backend/server.key" \
    > "$OTLP_WORK_DIR/logs/test_server.log" 2>&1 &
BACKEND_PID=$!
echo "$BACKEND_PID" > "$OTLP_WORK_DIR/pids/test_server.pid"
EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
    "$PROJECT_ROOT/target/debug/edgion-gateway" -c "$OTLP_WORK_DIR/config/gateway.yaml" \
    --work-dir "$OTLP_WORK_DIR" --integration-testing-mode \
    > "$OTLP_WORK_DIR/logs/gateway.log" 2>&1 &
GATEWAY_PID=$!
echo "$GATEWAY_PID" > "$OTLP_WORK_DIR/pids/gateway.pid"
python3 "$HELPER" run
# The run phase sends distinct traced records immediately before returning.
# A successful SDK teardown alone is not delivery evidence: verify the collector.
kill -TERM "$GATEWAY_PID"
python3 "$HELPER" wait-pid "$GATEWAY_PID" 75
wait "$GATEWAY_PID"
GATEWAY_PID=""
python3 "$HELPER" shutdown-delivery
