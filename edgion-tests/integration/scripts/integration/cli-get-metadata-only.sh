#!/usr/bin/env bash
# =============================================================================
# cli-get-metadata-only.sh
#
# Verifies `edgion-cli get --metadata-only` behavior (Card 05):
#   - Default `-o yaml` output contains spec:
#   - `--metadata-only -o yaml` output does NOT contain spec: (projection works)
#   - `--metadata-only` table mode still shows NAME column (regression guard)
#
# Boots its own controller in FS mode (no K8s required).
# Ports: admin=15834, gRPC=15835
# =============================================================================
set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC}   $1"; }
log_error()   { echo -e "${RED}[ERR]${NC}  $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() { echo; echo -e "${BLUE}=== $1 ===${NC}"; }

command -v python3 >/dev/null || { log_warn "python3 required; SKIP"; exit 0; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
UTILS_DIR="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
CLI="${PROJECT_ROOT}/target/debug/edgion-cli"
# shellcheck source=../utils/conf_sync_certs.sh
source "${UTILS_DIR}/conf_sync_certs.sh"

[[ -x "$CONTROLLER_BIN" ]] || { log_warn "edgion-controller not built; SKIP"; exit 0; }
[[ -x "$CLI" ]]            || { log_warn "edgion-cli not built; SKIP"; exit 0; }

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/cli_metadata_only_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
CONF_WATCH_DIR="${WORK_DIR}/conf"
LOG_DIR="${WORK_DIR}/logs"

CONTROLLER_ADMIN_PORT=15834
CONTROLLER_GRPC_PORT=15835
# /health and /ready live on the dedicated probe listener, not the admin API.
# Pin it (and the metrics listener) to test-specific ports so they do not fall
# back to the shared defaults (12100 / 12190) and collide with the main stack.
CONTROLLER_PROBE_PORT=15836
CONTROLLER_METRICS_PORT=15837
ADMIN_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"
PROBE_URL="http://127.0.0.1:${CONTROLLER_PROBE_PORT}"

CONTROLLER_PID=""
TEST_PASSED=0

cleanup() {
    log_section "Cleanup"
    if [[ -n "$CONTROLLER_PID" ]] && kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        kill "$CONTROLLER_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$CONTROLLER_PID" 2>/dev/null || true
    fi
    if [[ "$TEST_PASSED" -eq 1 ]]; then
        rm -rf "$WORK_DIR"
    else
        log_warn "Keeping WORK_DIR for post-mortem: $WORK_DIR"
    fi
}
trap cleanup EXIT

log_section "Setup"
mkdir -p "$CONFIG_DIR" "$CONF_WATCH_DIR" "$LOG_DIR"
ln -s "${PROJECT_ROOT}/config/crd" "${CONFIG_DIR}/crd"
# conf_sync is mTLS by default: put the certs where the emitted block expects them
# (work-dir relative), then let the shared emitter write the transport block.
conf_sync_distribute_certs "$WORK_DIR"

cat > "${CONFIG_DIR}/edgion-controller.yaml" <<EOF
work_dir: "${WORK_DIR}"

server:
  grpc_listen: "127.0.0.1:${CONTROLLER_GRPC_PORT}"
  admin_listen: "127.0.0.1:${CONTROLLER_ADMIN_PORT}"
  probe_listen: "127.0.0.1:${CONTROLLER_PROBE_PORT}"
  metrics_listen: "127.0.0.1:${CONTROLLER_METRICS_PORT}"

logging:
  log_dir: "${LOG_DIR}"
  log_prefix: edgion-controller
  log_level: info
  console: false

conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: "${CONF_WATCH_DIR}"

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
EOF
conf_sync_security_block server >> "${CONFIG_DIR}/edgion-controller.yaml"

# Seed 5 HTTPRoutes (route-1 through route-5)
NAMESPACE="ns-cli-meta"
FIXTURE_COUNT=5
mkdir -p "${CONF_WATCH_DIR}/${NAMESPACE}"
for i in $(seq 1 ${FIXTURE_COUNT}); do
  cat > "${CONF_WATCH_DIR}/${NAMESPACE}/HTTPRoute_${NAMESPACE}_route-${i}.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route-${i}
  namespace: ${NAMESPACE}
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /r${i}
EOF
done
log_info "Config + ${FIXTURE_COUNT} HTTPRoute fixtures written"

log_section "Start Controller (FileSystem mode)"
# Admin token for the Controller we start below (token-only model). It reads a
# single pre-shared bearer token from EDGION_ADMIN_TOKEN; honor an inherited value
# (e.g. from run_integration.sh) or fall back to the same fixed test token. The CLI
# reads the same value via EDGION_TOKEN.
export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
export EDGION_TOKEN="${EDGION_ADMIN_TOKEN}"
"$CONTROLLER_BIN" --config-file "${CONFIG_DIR}/edgion-controller.yaml" \
    > "${LOG_DIR}/controller.stdout.log" 2> "${LOG_DIR}/controller.stderr.log" &
CONTROLLER_PID=$!
log_info "Controller PID=$CONTROLLER_PID"

for i in $(seq 1 30); do
    if curl -sf "${PROBE_URL}/health" > /dev/null 2>&1; then
        log_success "Controller ready"
        break
    fi
    if ! kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_error "Controller exited during startup"
        tail -40 "${LOG_DIR}/controller.stderr.log" || true
        exit 1
    fi
    sleep 1
done

log_section "Authenticate"
# Token-only model: adopt the pre-shared admin token exported above as the Bearer
# credential — there is no login round-trip.
ADMIN_TOKEN="${EDGION_ADMIN_TOKEN}"
if [[ -z "$ADMIN_TOKEN" ]]; then
    log_error "EDGION_ADMIN_TOKEN not set — cannot authenticate"
    exit 1
fi
log_success "Got admin token"

# Save token to ~/.edgion/token so edgion-cli picks it up automatically.
# Back up and restore any pre-existing token so this test is non-destructive.
TOKEN_FILE="${HOME}/.edgion/token"
TOKEN_BACKUP=""
if [[ -f "$TOKEN_FILE" ]]; then
    TOKEN_BACKUP=$(cat "$TOKEN_FILE")
fi
mkdir -p "${HOME}/.edgion"
printf '%s' "$ADMIN_TOKEN" > "$TOKEN_FILE"
restore_token() {
    if [[ -n "$TOKEN_BACKUP" ]]; then
        printf '%s' "$TOKEN_BACKUP" > "$TOKEN_FILE"
    else
        rm -f "$TOKEN_FILE"
    fi
}
trap 'restore_token; cleanup' EXIT

# Allow informer to finish initial reconcile
sleep 2

log_section "Assert default -o yaml contains spec:"
OUT_DEFAULT=$("$CLI" --server "$ADMIN_URL" get httproute -o yaml 2>&1)
echo "$OUT_DEFAULT" | grep -q '^[[:space:]]*spec:' \
    || { log_error "Default yaml missing spec: line"; echo "$OUT_DEFAULT" | head -30; exit 1; }
log_success "Default -o yaml has spec"

log_section "Assert --metadata-only -o yaml does NOT contain spec:"
OUT_META=$("$CLI" --server "$ADMIN_URL" get httproute -o yaml --metadata-only 2>&1)
if echo "$OUT_META" | grep -q '^[[:space:]]*spec:'; then
    log_error "--metadata-only yaml still has spec — projection broken"
    echo "$OUT_META" | head -30
    exit 1
fi
log_success "--metadata-only -o yaml has no spec"

log_section "Assert --metadata-only table regression"
OUT_META_TABLE=$("$CLI" --server "$ADMIN_URL" get httproute --metadata-only 2>&1)
echo "$OUT_META_TABLE" | grep -q "NAME" \
    || { log_error "--metadata-only table missing NAME header"; echo "$OUT_META_TABLE"; exit 1; }
echo "$OUT_META_TABLE" | grep -qE 'route-[0-9]+' \
    || { log_error "--metadata-only table missing fixture names"; echo "$OUT_META_TABLE"; exit 1; }
log_success "--metadata-only table OK"

TEST_PASSED=1
log_section "PASS: cli-get-metadata-only.sh"
