#!/bin/bash
# =============================================================================
# Stopall Edgion Testservice
# =============================================================================

set -e

# 
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# projectdirectory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# =============================================================================
# log
# =============================================================================
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

# =============================================================================
# Stopprocess
# =============================================================================
force_kill() {
    local pattern=$1
    local service_name=$2

    if ! pgrep -f "$pattern" > /dev/null 2>&1; then
        return
    fi

    # Send SIGTERM first so the process runs atexit handlers (LLVM coverage profraw
    # flush relies on this — pkill -9 / SIGKILL bypasses atexit and loses coverage data).
    pkill -TERM -f "$pattern" 2>/dev/null || true
    log_info "Stop $service_name (SIGTERM)"

    # Wait up to 5s for graceful exit; escalate to SIGKILL if still alive.
    local waited=0
    while [ $waited -lt 5 ] && pgrep -f "$pattern" > /dev/null 2>&1; do
        sleep 1
        waited=$((waited + 1))
    done

    if pgrep -f "$pattern" > /dev/null 2>&1; then
        pkill -9 -f "$pattern" 2>/dev/null || true
        log_info "Escalated $service_name (SIGKILL after ${waited}s)"
    fi
}

# =============================================================================
# 
# =============================================================================
main() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Stop Edgion Testservice${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
    
    # ShowWorkdirectory（）
    local current_file="${PROJECT_ROOT}/integration_testing/.current"
    if [ -f "$current_file" ]; then
        log_info "Workdirectory: $(cat "$current_file")"
    fi
    
    # Stopallprocess
    force_kill "edgion-gateway" "edgion-gateway"
    force_kill "edgion-controller" "edgion-controller"
    force_kill "edgion-center" "edgion-center"
    force_kill "test_server" "test_server"
    
    # Waitprocess
    sleep 1
    
    echo ""
    log_success "allservicealreadyStop"
}

main "$@"
