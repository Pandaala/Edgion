#!/bin/bash
# =============================================================================
# Stopall Edgion Testservice
# =============================================================================

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# project根directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

# =============================================================================
# log函数
# =============================================================================
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

# =============================================================================
# 强制Stopprocess
# =============================================================================
force_kill() {
    local pattern=$1
    local service_name=$2
    
    if pgrep -f "$pattern" > /dev/null 2>&1; then
        pkill -9 -f "$pattern" 2>/dev/null || true
        log_info "Stop $service_name"
    fi
}

# =============================================================================
# 主函数
# =============================================================================
main() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Stop Edgion Testservice${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
    
    # ShowWorkdirectory（如果有）
    local current_file="${PROJECT_ROOT}/integration_testing/.current"
    if [ -f "$current_file" ]; then
        log_info "Workdirectory: $(cat "$current_file")"
    fi
    
    # 强制Stopall相关process
    force_kill "edgion-gateway" "edgion-gateway"
    force_kill "edgion-controller" "edgion-controller"
    force_kill "test_server" "test_server"
    
    # Waitprocess完全退出
    sleep 1
    
    echo ""
    log_success "allservicealreadyStop"
}

main "$@"
