#!/bin/bash
# =============================================================================
# conf_sync gRPC mTLS certificate generator for integration tests.
#
# Uses `edgion-ctl gen-certs --output file` to produce:
#   ca.crt, server.crt, server.key, client.crt, client.key
#
# Output directory:
#   ${EDGION_CONF_SYNC_CERTS_DIR:-$PROJECT_ROOT/examples/test/certs/conf-sync-grpc}
#
# The script is idempotent: skips generation when all five files already exist
# (unless --force is passed).
# =============================================================================

set -e

BLUE='\033[0;34m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warning() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error()   { echo -e "${RED}[ERR]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CERTS_DIR="${EDGION_CONF_SYNC_CERTS_DIR:-$PROJECT_ROOT/examples/test/certs/conf-sync-grpc}"
EDGION_CTL="${PROJECT_ROOT}/target/debug/edgion-ctl"

FORCE=false
for arg in "$@"; do
    if [ "$arg" = "--force" ]; then
        FORCE=true
    fi
done

all_certs_exist() {
    local files=("ca.crt" "server.crt" "server.key" "client.crt" "client.key")
    for f in "${files[@]}"; do
        if [ ! -f "$CERTS_DIR/$f" ]; then
            return 1
        fi
    done
    return 0
}

log_info "conf_sync gRPC mTLS certificate generator"
log_info "Output directory: $CERTS_DIR"

if ! $FORCE && all_certs_exist; then
    log_info "All conf_sync mTLS certificates already exist, reusing."
    exit 0
fi

if [ ! -f "$EDGION_CTL" ]; then
    log_error "edgion-ctl not found at $EDGION_CTL — run prepare.sh first"
    exit 1
fi

mkdir -p "$CERTS_DIR"

log_info "Generating conf_sync mTLS certificates via edgion-ctl gen-certs..."
# localhost, 127.0.0.1, ::1 are already included by default in gen-certs
"$EDGION_CTL" gen-certs \
    --output file \
    --output-dir "$CERTS_DIR" \
    --ca-days 3650 \
    --cert-days 365 \
    --force

if ! all_certs_exist; then
    log_error "Certificate generation completed but some files are missing"
    ls -la "$CERTS_DIR"
    exit 1
fi

log_success "conf_sync mTLS certificates generated:"
log_info "  $CERTS_DIR/ca.crt      (CA certificate)"
log_info "  $CERTS_DIR/server.crt  (Controller server cert)"
log_info "  $CERTS_DIR/server.key  (Controller server key)"
log_info "  $CERTS_DIR/client.crt  (Gateway client cert)"
log_info "  $CERTS_DIR/client.key  (Gateway client key)"
