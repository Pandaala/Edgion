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

source "${SCRIPT_DIR}/artifact_paths.sh"

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
shutdown_wait_seconds() {
    local service_name=$1

    if [ "${EDGION_COVERAGE:-0}" = "1" ] && [ "$service_name" = "edgion-gateway" ]; then
        echo 60
    else
        echo 5
    fi
}

force_kill() {
    local pattern=$1
    local service_name=$2
    local wait_seconds=$3

    if ! pgrep -f "$pattern" > /dev/null 2>&1; then
        return
    fi

    # Send SIGTERM first so the process runs atexit handlers (LLVM coverage profraw
    # flush relies on this — pkill -9 / SIGKILL bypasses atexit and loses coverage data).
    pkill -TERM -f "$pattern" 2>/dev/null || true
    log_info "Stop $service_name (SIGTERM)"

    # Coverage needs the Gateway to pass Pingora's normal shutdown window and
    # flush its LLVM profile. Every path remains bounded, and ordinary runs keep
    # the short teardown window.
    local waited=0
    while [ "$waited" -lt "$wait_seconds" ] && pgrep -f "$pattern" > /dev/null 2>&1; do
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

    process_matches_work_dir "$pid" "${work_dir}/logs/ext_proc_external_processor.log" || return 0
    kill -TERM "$pid" 2>/dev/null || true
    log_info "Stop external ExtProc fixture PID ${pid} (SIGTERM)"
    local waited=0
    while [ "$waited" -lt 5 ] && kill -0 "$pid" 2>/dev/null; do
        sleep 1
        waited=$((waited + 1))
    done
    if kill -0 "$pid" 2>/dev/null && process_matches_binary "$pid" "$expected" \
        && process_matches_work_dir "$pid" "${work_dir}/logs/ext_proc_external_processor.log"; then
        kill -KILL "$pid" 2>/dev/null || true
        log_info "Escalated external ExtProc fixture PID ${pid} (SIGKILL)"
    fi
}

# The startup scripts redirect stdout to an invocation-owned log before exec.
# Unlike a PID plus executable, this also distinguishes another live instance.
process_matches_work_dir() {
    local pid="$1" expected_log="$2" actual_log=""
    if [ -e "/proc/${pid}/fd/1" ]; then
        actual_log=$(readlink "/proc/${pid}/fd/1" 2>/dev/null || true)
    elif command -v lsof >/dev/null 2>&1; then
        actual_log=$(lsof -a -p "$pid" -d 1 -Fn 2>/dev/null | sed -n 's/^n//p' | head -1)
    fi
    [ "$actual_log" = "$expected_log" ]
}

# Explicit runner cleanup never scans or signals another topology.
stop_recorded_service() {
    local work_dir="$1" pid_name="$2" binary_name="$3"
    local pid_file="${work_dir}/pids/${pid_name}.pid"
    [ -f "$pid_file" ] && [ ! -L "$pid_file" ] || return 0
    local pid expected waited
    pid=$(cat "$pid_file")
    case "$pid" in ''|*[!0-9]*) return 0 ;; esac
    [ "$pid" -gt 1 ] || return 0
    expected="${PROJECT_ROOT}/target/debug/${binary_name}"
    if ! process_matches_binary "$pid" "$expected"; then
        expected="${PROJECT_ROOT}/target/llvm-cov-target/debug/${binary_name}"
        process_matches_binary "$pid" "$expected" || return 0
    fi
    process_matches_work_dir "$pid" "${work_dir}/logs/${pid_name}.log" || return 0
    kill -TERM "$pid" 2>/dev/null || true
    waited=0
    local limit
    limit=$(shutdown_wait_seconds "$binary_name")
    while [ "$waited" -lt "$limit" ] && kill -0 "$pid" 2>/dev/null; do
        sleep 1
        waited=$((waited + 1))
    done
    if kill -0 "$pid" 2>/dev/null && process_matches_binary "$pid" "$expected" \
        && process_matches_work_dir "$pid" "${work_dir}/logs/${pid_name}.log"; then
        kill -KILL "$pid" 2>/dev/null || true
    fi
}

cleanup_work_dir() {
    local work_dir="$1" suffix
    case "$work_dir" in "${PROJECT_ROOT}/tmp/integration_testing/testing_"*) ;; *) return 2 ;; esac
    suffix=${work_dir#"${PROJECT_ROOT}/tmp/integration_testing/testing_"}
    case "$suffix" in ''|*[!0-9_]*) return 2 ;; esac
    [ -d "$work_dir" ] && [ ! -L "$work_dir" ] || return 2
    log_info "Workdirectory: ${work_dir}"
    # Stop the owner first so cleanup cannot race a pending replacement.
    python3 "${SCRIPT_DIR}/restart_controller.py" --stop "$work_dir" || return $?
    stop_recorded_service "$work_dir" gateway edgion-gateway
    stop_recorded_service "$work_dir" controller edgion-controller
    stop_recorded_service "$work_dir" test_server test_server
    local external_pid_file="${work_dir}/pids/ext_proc_external_processor.pid"
    if [ -f "$external_pid_file" ] && [ ! -L "$external_pid_file" ]; then
        local external_pid
        external_pid=$(cat "$external_pid_file")
        case "$external_pid" in ''|*[!0-9]*) return 0 ;; esac
        if process_matches_work_dir "$external_pid" "${work_dir}/logs/ext_proc_external_processor.log"; then
            stop_recorded_external_ext_proc "$work_dir"
        fi
    fi
}

stop_owned_release() {
    local work_dir="${EDGION_TEST_OWNED_WORK_DIR:-}"
    edgion_validate_owned_work_dir "$work_dir" || return 2
    [ -f "$work_dir/artifact-profile" ] || return 0
    [ "$(cat "$work_dir/artifact-profile")" = release ] || return 2
    python3 "${SCRIPT_DIR}/restart_controller.py" --stop "$work_dir" || return $?
    local role binary pid pid_file waited failed=0
    for role in gateway controller test_server; do
        case "$role" in
            gateway) binary=edgion-gateway ;;
            controller) binary=edgion-controller ;;
            test_server) binary=test_server ;;
        esac
        binary="${PROJECT_ROOT}/target/release/${binary}"
        pid_file="$work_dir/pids/$role.pid"
        [ -f "$pid_file" ] || continue
        pid=$(cat "$pid_file")
        case "$pid" in ''|*[!0-9]*) failed=1; continue ;; esac
        if [ "$pid" -le 1 ]; then failed=1; continue; fi
        kill -0 "$pid" 2>/dev/null || continue
        if ! process_matches_binary "$pid" "$binary"; then
            log_warning "Owned PID $pid has changed identity; refusing to signal"
            failed=1
            continue
        fi
        if ! process_matches_work_dir "$pid" "$work_dir/logs/$role.log"; then
            log_warning "Owned PID $pid has changed work directory; refusing to signal"
            failed=1
            continue
        fi
        kill -TERM "$pid" 2>/dev/null || { failed=1; continue; }
        waited=0
        while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt 5 ]; do
            sleep 1
            waited=$((waited + 1))
        done
        if kill -0 "$pid" 2>/dev/null; then
            if ! process_matches_binary "$pid" "$binary" \
                || ! process_matches_work_dir "$pid" "$work_dir/logs/$role.log"; then
                failed=1
                continue
            fi
            kill -KILL "$pid" 2>/dev/null || failed=1
            sleep 1
            if kill -0 "$pid" 2>/dev/null; then failed=1; fi
        fi
    done
    return "$failed"
}

# =============================================================================
# 
# =============================================================================
main() {
    edgion_resolve_artifacts || return $?
    if [ "${1:-}" = "--work-dir" ] && [ "$#" -eq 2 ]; then
        if [ "$EDGION_TEST_PROFILE" = release ]; then
            [ "$2" = "${EDGION_TEST_OWNED_WORK_DIR:-}" ] || return 2
            stop_owned_release
        else
            cleanup_work_dir "$2"
        fi
        return $?
    fi
    if [ "$#" -ne 0 ]; then
        log_warning "Usage: kill_all.sh [--work-dir <integration work directory>]"
        return 2
    fi
    if [ "$EDGION_TEST_PROFILE" = release ]; then
        stop_owned_release
        return $?
    fi
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
                python3 "${SCRIPT_DIR}/restart_controller.py" --stop "$current_work_dir" || return $?
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
    force_kill "debug/edgion-gateway" "edgion-gateway" "$(shutdown_wait_seconds "edgion-gateway")"
    force_kill "debug/edgion-controller" "edgion-controller" "$(shutdown_wait_seconds "edgion-controller")"
    force_kill "debug/test_server" "test_server" "$(shutdown_wait_seconds "test_server")"
    # Anchored fallback covers startup failures before .current is published.
    # It cannot match go build/compiler commands or an unrelated similarly
    # named process because the absolute executable path starts the command.
    force_kill "^${PROJECT_ROOT}/target/debug/ext_proc_external_processor([[:space:]]|$)" \
        "external ExtProc fixture" "$(shutdown_wait_seconds "external ExtProc fixture")"
    force_kill "^${PROJECT_ROOT}/target/llvm-cov-target/debug/ext_proc_external_processor([[:space:]]|$)" \
        "external ExtProc fixture (coverage)" "$(shutdown_wait_seconds "external ExtProc fixture (coverage)")"
    
    # Waitprocess
    sleep 1
    
    echo ""
    log_success "allservicealreadyStop"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then main "$@"; fi
