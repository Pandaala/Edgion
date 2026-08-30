#!/bin/bash
# =============================================================================
# conf_sync gRPC mTLS certificate generator for integration tests.
#
# Uses `edgion-cli gen-certs --output file --cert-name <name>` to produce
# two named cert sets (primary and secondary) under:
#   ${CERTS_DIR}/primary/{ca.crt,server.crt,server.key,client.crt,client.key}
#   ${CERTS_DIR}/secondary/{ca.crt,server.crt,server.key,client.crt,client.key}
#
# Output directory: EDGION_CONF_SYNC_CERTS_DIR (REQUIRED). The caller owns the
# output directory — normally <work_dir>/certs/grpc of the process about to be
# started (conf_sync_distribute_certs sets this). There is deliberately no
# default: an implicit shared store outside the per-run work directory is
# exactly the repo-root/private-key-residue bug this contract prevents.
#
# The script is idempotent: skips generation when all files already exist
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
if [ -z "${EDGION_CONF_SYNC_CERTS_DIR:-}" ]; then
    echo -e "\033[0;31m[ERR]\033[0m EDGION_CONF_SYNC_CERTS_DIR must be set to a caller-owned output directory (e.g. <work_dir>/certs/grpc)" >&2
    exit 1
fi
CERTS_DIR="$EDGION_CONF_SYNC_CERTS_DIR"
EDGION_CLI="${EDGION_CLI_BIN:-${PROJECT_ROOT}/target/debug/edgion-cli}"

FORCE=false
for arg in "$@"; do
    if [ "$arg" = "--force" ]; then
        FORCE=true
    fi
done

# Check if all files for a named cert set exist
cert_set_exists() {
    local name="$1"
    local files=("ca.crt" "server.crt" "server.key" "client.crt" "client.key")
    for f in "${files[@]}"; do
        if [ ! -f "$CERTS_DIR/$name/$f" ]; then
            return 1
        fi
    done
    return 0
}

all_certs_exist() {
    cert_set_exists "primary" && cert_set_exists "secondary"
}

log_info "conf_sync gRPC mTLS certificate generator"
log_info "Output directory: $CERTS_DIR"

if ! $FORCE && all_certs_exist; then
    log_info "All conf_sync mTLS certificates already exist, reusing."
    exit 0
fi

if [ ! -f "$EDGION_CLI" ]; then
    log_error "edgion-cli not found at $EDGION_CLI — run prepare.sh first"
    exit 1
fi

mkdir -p "$CERTS_DIR"

# Generate primary cert set
log_info "Generating 'primary' conf_sync mTLS certificates via edgion-cli gen-certs..."
"$EDGION_CLI" gen-certs \
    --output file \
    --output-dir "$CERTS_DIR" \
    --cert-name primary \
    --ca-days 3650 \
    --cert-days 365 \
    --force

if ! cert_set_exists "primary"; then
    log_error "Primary certificate generation completed but some files are missing"
    ls -la "$CERTS_DIR/primary/" 2>/dev/null || true
    exit 1
fi
log_success "Primary cert set generated: $CERTS_DIR/primary/"

# Generate secondary cert set (independent CA — used to verify multi-cert config)
log_info "Generating 'secondary' conf_sync mTLS certificates via edgion-cli gen-certs..."
"$EDGION_CLI" gen-certs \
    --output file \
    --output-dir "$CERTS_DIR" \
    --cert-name secondary \
    --ca-days 3650 \
    --cert-days 365 \
    --force

if ! cert_set_exists "secondary"; then
    log_error "Secondary certificate generation completed but some files are missing"
    ls -la "$CERTS_DIR/secondary/" 2>/dev/null || true
    exit 1
fi
log_success "Secondary cert set generated: $CERTS_DIR/secondary/"

log_success "conf_sync mTLS certificates generated (primary + secondary):"
log_info "  $CERTS_DIR/primary/ca.crt      (CA certificate)"
log_info "  $CERTS_DIR/primary/server.crt  (Controller server cert)"
log_info "  $CERTS_DIR/primary/server.key  (Controller server key)"
log_info "  $CERTS_DIR/primary/client.crt  (Gateway client cert)"
log_info "  $CERTS_DIR/primary/client.key  (Gateway client key)"
log_info "  $CERTS_DIR/secondary/ca.crt      (CA certificate)"
log_info "  $CERTS_DIR/secondary/server.crt  (Controller server cert)"
log_info "  $CERTS_DIR/secondary/server.key  (Controller server key)"
log_info "  $CERTS_DIR/secondary/client.crt  (Gateway client cert)"
log_info "  $CERTS_DIR/secondary/client.key  (Gateway client key)"
