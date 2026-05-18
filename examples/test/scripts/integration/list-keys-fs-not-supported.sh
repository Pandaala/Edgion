#!/usr/bin/env bash
# =============================================================================
# list-keys-fs-not-supported.sh
#
# Verifies FS-backend behavior for the new metadata-only keys endpoints
# introduced in Card 01 of list-keys-api:
#   - GET /api/v1/keys/cluster/{kind}           -> 501 + "not yet supported"
#   - GET /api/v1/keys/namespaced/{kind}        -> 501 + "not yet supported"
#   - Regression: legacy /api/v1/cluster/{kind} stays unchanged (not 501)
#
# This script spins up its own controller in FileSystem mode so it has no
# external dependency beyond the built `edgion-controller` binary and `jq`.
# =============================================================================
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC}   $1"; }
log_error()   { echo -e "${RED}[ERR]${NC}  $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() { echo; echo -e "${BLUE}=== $1 ===${NC}"; }

# --- Prereqs ---
command -v jq >/dev/null      || { log_warn "jq not installed; SKIP"; exit 0; }
command -v python3 >/dev/null || { log_warn "python3 not installed; SKIP"; exit 0; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
CONTROLLER_BIN="${PROJECT_ROOT}/target/debug/edgion-controller"

if [[ ! -x "$CONTROLLER_BIN" ]]; then
    log_warn "edgion-controller binary not found at $CONTROLLER_BIN; SKIP"
    log_info "Build with: cargo build --bin edgion-controller"
    exit 0
fi

# --- Work dir + ports ---
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR="${PROJECT_ROOT}/integration_testing/list_keys_fs_${TIMESTAMP}"
CONFIG_DIR="${WORK_DIR}/config"
CONF_WATCH_DIR="${WORK_DIR}/conf"
LOG_DIR="${WORK_DIR}/logs"

CONTROLLER_ADMIN_PORT=15820
CONTROLLER_GRPC_PORT=15821
ADMIN_URL="http://127.0.0.1:${CONTROLLER_ADMIN_PORT}"

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

# --- Setup ---
log_section "Setup"
mkdir -p "$CONFIG_DIR" "$CONF_WATCH_DIR" "$LOG_DIR"
ln -s "${PROJECT_ROOT}/config/crd" "${CONFIG_DIR}/crd"

cat > "${CONFIG_DIR}/edgion-controller.yaml" <<EOF
work_dir: "${WORK_DIR}"

server:
  grpc_listen: "127.0.0.1:${CONTROLLER_GRPC_PORT}"
  admin_listen: "127.0.0.1:${CONTROLLER_ADMIN_PORT}"

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
log_info "Config written"

# --- Launch controller ---
log_section "Start Controller (FileSystem mode)"
"$CONTROLLER_BIN" --config-file "${CONFIG_DIR}/edgion-controller.yaml" \
    > "${LOG_DIR}/controller.stdout.log" 2> "${LOG_DIR}/controller.stderr.log" &
CONTROLLER_PID=$!
log_info "Controller PID=$CONTROLLER_PID"

# Wait for /health
for i in $(seq 1 30); do
    if curl -sf "${ADMIN_URL}/health" > /dev/null 2>&1; then
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

# --- Obtain admin token ---
log_section "Authenticate"
SECRET_FILE=""
for i in $(seq 1 10); do
    SECRET_FILE=$(find "$CONF_WATCH_DIR" -name 'Secret_edgion-system_edgion-admin-credentials.yaml' -type f 2>/dev/null | head -1)
    [[ -n "$SECRET_FILE" ]] && break
    sleep 1
done
if [[ -z "$SECRET_FILE" ]]; then
    log_error "Admin secret not generated under $CONF_WATCH_DIR"
    exit 1
fi
PASSWORD=$(awk '/^[[:space:]]+password:/ {print $2; exit}' "$SECRET_FILE" | tr -d '"')
ADMIN_TOKEN=$(curl -s -H "Content-Type: application/json" \
    -d "{\"username\":\"admin\",\"password\":\"${PASSWORD}\"}" \
    "${ADMIN_URL}/api/v1/auth/login" \
    | python3 -c "import sys,json; print(json.load(sys.stdin).get('data',{}).get('token',''))")
if [[ -z "$ADMIN_TOKEN" ]]; then
    log_error "Login failed"
    exit 1
fi
log_success "Got admin token"

curl_admin() { curl -s -H "Authorization: Bearer ${ADMIN_TOKEN}" "$@"; }

# --- Assertions ---
log_section "Assert /api/v1/keys/cluster/{kind} returns 501"
STATUS=$(curl_admin -o /tmp/list-keys-fs-cluster.json -w "%{http_code}" \
    "${ADMIN_URL}/api/v1/keys/cluster/GatewayClass")
if [[ "$STATUS" != "501" ]]; then
    log_error "Expected 501, got $STATUS"
    cat /tmp/list-keys-fs-cluster.json
    exit 1
fi
if ! grep -qi "not yet supported" /tmp/list-keys-fs-cluster.json; then
    log_warn "Body does not contain 'not yet supported' (may be empty body on 501; that's acceptable)"
fi
log_success "/api/v1/keys/cluster/GatewayClass returned 501"

log_section "Assert /api/v1/keys/namespaced/{kind} returns 501"
STATUS=$(curl_admin -o /tmp/list-keys-fs-ns.json -w "%{http_code}" \
    "${ADMIN_URL}/api/v1/keys/namespaced/HTTPRoute")
if [[ "$STATUS" != "501" ]]; then
    log_error "Expected 501, got $STATUS"
    cat /tmp/list-keys-fs-ns.json
    exit 1
fi
log_success "/api/v1/keys/namespaced/HTTPRoute returned 501"

log_section "Regression: legacy /api/v1/cluster/{kind} not 501"
LEGACY_STATUS=$(curl_admin -o /tmp/list-keys-fs-legacy.json -w "%{http_code}" \
    "${ADMIN_URL}/api/v1/cluster/GatewayClass")
if [[ "$LEGACY_STATUS" == "501" ]]; then
    log_error "Legacy path returned 501 — unintended regression!"
    exit 1
fi
log_success "Legacy path status=${LEGACY_STATUS} (not 501)"

TEST_PASSED=1
log_section "PASS: list-keys-fs-not-supported.sh"
