#!/bin/bash
# =============================================================================
# ACME Integration Test Script
#
# Starts Pebble (ACME test CA) + challtestsrv, runs ACME integration tests,
# and cleans up.
#
# Usage:
#   ./run_acme_test.sh              # Run all ACME tests
#   ./run_acme_test.sh dns01        # Run only DNS-01 test
#   ./run_acme_test.sh --no-cleanup # Keep Pebble running after tests
# =============================================================================

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
PEBBLE_DIR="${PROJECT_ROOT}/examples/test/conf/Services/acme/pebble"

DO_CLEANUP=true
TEST_FILTER=""

# Parse args
for arg in "$@"; do
    case $arg in
        --no-cleanup)
            DO_CLEANUP=false
            ;;
        *)
            TEST_FILTER="$arg"
            ;;
    esac
done

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error()   { echo -e "${RED}[✗]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[!]${NC} $1"; }

# =============================================================================
# Cleanup
# =============================================================================
cleanup() {
    if [ "$DO_CLEANUP" = true ]; then
        log_info "Stopping Pebble environment..."
        cd "$PEBBLE_DIR" && docker compose down --timeout 5 2>/dev/null || true
        log_info "Cleanup done"
    else
        log_warn "Pebble still running (--no-cleanup). Stop with:"
        log_warn "  cd $PEBBLE_DIR && docker compose down"
    fi
}

trap cleanup EXIT

# =============================================================================
# Start Pebble
# =============================================================================
log_info "Starting Pebble ACME test environment..."

cd "$PEBBLE_DIR"

# Pull images if needed
docker compose pull --quiet 2>/dev/null || true

# Start services
docker compose up -d

# Wait for Pebble to be ready
log_info "Waiting for Pebble to be ready..."
MAX_WAIT=30
for i in $(seq 1 $MAX_WAIT); do
    if curl -sk https://localhost:14000/dir > /dev/null 2>&1; then
        log_success "Pebble is ready (waited ${i}s)"
        break
    fi
    if [ "$i" -eq "$MAX_WAIT" ]; then
        log_error "Pebble failed to start within ${MAX_WAIT}s"
        docker compose logs
        exit 1
    fi
    sleep 1
done

# Also check challtestsrv
if curl -s http://localhost:8055 > /dev/null 2>&1; then
    log_success "challtestsrv is ready"
else
    log_warn "challtestsrv may not be ready yet, continuing..."
fi

# Extract Pebble's TLS CA certificate (needed for instant-acme to trust Pebble)
log_info "Extracting Pebble TLS CA certificate..."
rm -f /tmp/pebble-minica-ca.pem
docker cp pebble-pebble-1:/test/certs/pebble.minica.pem /tmp/pebble-minica-ca.pem 2>/dev/null
if [ -f /tmp/pebble-minica-ca.pem ]; then
    log_success "Pebble TLS CA extracted to /tmp/pebble-minica-ca.pem"
else
    log_warn "Could not extract Pebble TLS CA (will be extracted by test code)"
fi

# =============================================================================
# Build & Run tests via unified test framework
# =============================================================================
cd "$PROJECT_ROOT"

log_info "Building test client..."
cargo build --example test_client 2>&1 | tail -3
if [ $? -ne 0 ]; then
    log_error "Failed to build test_client"
    exit 1
fi

log_info "Running ACME integration tests..."
echo ""

TEST_CMD="cargo run --example test_client -- -r Services -i acme"

echo "$ $TEST_CMD"
echo ""

if eval "$TEST_CMD"; then
    echo ""
    log_success "All ACME integration tests passed!"
    EXIT_CODE=0
else
    echo ""
    log_error "Some ACME integration tests failed!"
    EXIT_CODE=1
fi

exit $EXIT_CODE
