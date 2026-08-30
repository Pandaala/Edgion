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

log_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
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

process_matches_binary() {
    local pid="$1"
    local expected="$2"
    local actual=""

    if [ -e "/proc/${pid}/exe" ]; then
        actual=$(readlink "/proc/${pid}/exe" 2>/dev/null || true)
    elif command -v lsof >/dev/null 2>&1; then
        actual=$(lsof -a -p "$pid" -d txt -Fn 2>/dev/null | sed -n 's/^n//p' | head -1)
    else
        local command_line
        command_line=$(ps -p "$pid" -o command= 2>/dev/null || true)
        case "$command_line" in
            "$expected"|"$expected "*) actual="$expected" ;;
        esac
    fi

    [ "$actual" = "$expected" ]
}

stop_recorded_external_ext_proc() {
    local work_dir="$1"
    local pid_file="${work_dir}/pids/ext_proc_external_processor.pid"
    local binary_file="${work_dir}/ext-proc-external-bin"
    [ -f "$pid_file" ] && [ -f "$binary_file" ] || return 0

    local pid expected
    pid=$(cat "$pid_file")
    expected=$(cat "$binary_file")
    case "$pid" in
        ''|*[!0-9]*)
            log_warning "Ignoring invalid external ExtProc PID file: ${pid_file}"
            return 0
            ;;
    esac
    case "$expected" in
        "${PROJECT_ROOT}/target/debug/ext_proc_external_processor"|\
        "${PROJECT_ROOT}/target/llvm-cov-target/debug/ext_proc_external_processor") ;;
        *)
            log_warning "Ignoring unexpected external ExtProc binary path: ${expected}"
            return 0
            ;;
    esac
    if ! kill -0 "$pid" 2>/dev/null; then
        return 0
    fi
    if ! process_matches_binary "$pid" "$expected"; then
        log_warning "External ExtProc PID ${pid} no longer matches ${expected}; refusing to signal it"
        return 0
    fi

    kill -TERM "$pid" 2>/dev/null || true
    log_info "Stop external ExtProc fixture PID ${pid} (SIGTERM)"
    local waited=0
    while [ "$waited" -lt 5 ] && kill -0 "$pid" 2>/dev/null; do
        sleep 1
        waited=$((waited + 1))
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null || true
        log_info "Escalated external ExtProc fixture PID ${pid} (SIGKILL)"
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
    local current_file="${PROJECT_ROOT}/tmp/integration_testing/.current"
    local current_work_dir=""
    if [ -f "$current_file" ]; then
        current_work_dir=$(cat "$current_file")
        case "$current_work_dir" in
            "${PROJECT_ROOT}/tmp/integration_testing/"*)
                log_info "Workdirectory: ${current_work_dir}"
                stop_recorded_external_ext_proc "$current_work_dir"
                ;;
            *)
                log_warning "Ignoring unexpected work directory marker: ${current_work_dir}"
                ;;
        esac
    fi
    
    # Stopallprocess
    # Anchor the match on the built binary path (target/{debug,llvm-cov-target/debug}/<bin>)
    # so an in-flight `cargo test -p edgion-gateway` / `rustc ... edgion-gateway/src` is not
    # matched and SIGKILLed. The second arg stays the friendly name for log output.
    force_kill "debug/edgion-gateway" "edgion-gateway"
    force_kill "debug/edgion-controller" "edgion-controller"
    force_kill "debug/test_server" "test_server"
    # Anchored fallback covers startup failures before .current is published.
    # It cannot match go build/compiler commands or an unrelated similarly
    # named process because the absolute executable path starts the command.
    force_kill "^${PROJECT_ROOT}/target/debug/ext_proc_external_processor([[:space:]]|$)" "external ExtProc fixture"
    force_kill "^${PROJECT_ROOT}/target/llvm-cov-target/debug/ext_proc_external_processor([[:space:]]|$)" "external ExtProc fixture (coverage)"
    
    # Waitprocess
    sleep 1
    
    echo ""
    log_success "allservicealreadyStop"
}

main "$@"
