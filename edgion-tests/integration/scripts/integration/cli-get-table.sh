#!/usr/bin/env bash
# =============================================================================
# cli-get-table.sh
#
# Verifies `edgion-cli get` table mode output (Card 05):
#   - Default table includes NAME / NAMESPACE / KIND / AGE columns
#   - `-o wide` adds LABELS / RESOURCE_VERSION columns
#
# Boots its own controller in FS mode (no K8s required).
# Ports: admin=15830, gRPC=15831
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
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"
CLI="${PROJECT_ROOT}/target/debug/edgion-cli"

[[ -x "$CONTROLLER_BIN" ]] || { log_warn "edgion-controller not built; SKIP"; exit 0; }
[[ -x "$CLI" ]]            || { log_warn "edgion-cli not built; SKIP"; exit 0; }

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/cli_get_table_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
CONF_WATCH_DIR="${WORK_DIR}/conf"
LOG_DIR="${WORK_DIR}/logs"

CONTROLLER_ADMIN_PORT=15830
CONTROLLER_GRPC_PORT=15831
# /health and /ready live on the dedicated probe listener, not the admin API.
# Pin it (and the metrics listener) to test-specific ports so they do not fall
# back to the shared defaults (12100 / 12190) and collide with the main stack.
CONTROLLER_PROBE_PORT=15832
CONTROLLER_METRICS_PORT=15833
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
  conf_dir: "${CONF_WATCH_DIR}"

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
EOF

# Seed one HTTPRoute with non-empty labels so the wide-mode columns are exercised.
NAMESPACE="ns-cli-table"
mkdir -p "${CONF_WATCH_DIR}/${NAMESPACE}"
cat > "${CONF_WATCH_DIR}/${NAMESPACE}/HTTPRoute_${NAMESPACE}_route-1.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route-1
  namespace: ${NAMESPACE}
  labels:
    app: demo
    env: test
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /r1
EOF
log_info "Config + 1 HTTPRoute fixture written"

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

log_section "Assert default table has NAME / NAMESPACE / KIND / AGE columns"
OUT=$("$CLI" --server "$ADMIN_URL" get httproute 2>&1)
echo "$OUT" | grep -q "NAME"       || { log_error "missing NAME header"; echo "$OUT"; exit 1; }
echo "$OUT" | grep -q "NAMESPACE"  || { log_error "missing NAMESPACE header"; echo "$OUT"; exit 1; }
echo "$OUT" | grep -q "KIND"       || { log_error "missing KIND header"; echo "$OUT"; exit 1; }
echo "$OUT" | grep -q "AGE"        || { log_error "missing AGE header"; echo "$OUT"; exit 1; }
log_success "Default table columns OK"

log_section "Assert -o wide adds LABELS / RESOURCE_VERSION"
OUT_WIDE=$("$CLI" --server "$ADMIN_URL" get httproute -o wide 2>&1)
echo "$OUT_WIDE" | grep -q "LABELS"           || { log_error "wide missing LABELS"; echo "$OUT_WIDE"; exit 1; }
echo "$OUT_WIDE" | grep -q "RESOURCE_VERSION" || { log_error "wide missing RESOURCE_VERSION"; echo "$OUT_WIDE"; exit 1; }
log_success "-o wide columns OK"

TEST_PASSED=1
log_section "PASS: cli-get-table.sh"
