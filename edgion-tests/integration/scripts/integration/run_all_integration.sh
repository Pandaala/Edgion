#!/bin/bash
# =============================================================================
# Edgion — run every integration test (sequential orchestrator)
#
# Wraps the main runner plus every standalone integration script so contributors
# (and CI) have a single entry point. Each child manages its own services and
# its own teardown — this script only sequences them, captures their logs, and
# emits a unified report.
#
# All children currently share controller/gateway ports (12101/12001) so they
# MUST run sequentially. test_file_watcher.sh uses different ports but is still
# sequenced for predictable resource usage.
#
# Usage:
#   run_all_integration.sh                   # run every job, fast mode
#   run_all_integration.sh --full-test       # also run slow suites in main
#   run_all_integration.sh --no-prepare      # skip top-level cargo build
#   run_all_integration.sh --skip acme,cli   # exclude specific jobs
#   run_all_integration.sh --only main,linksys_etcd
#   run_all_integration.sh --job-timeout-secs 3600 --run-timeout-secs 21600
#   run_all_integration.sh --help
#
# Behaviour:
#   - Continue on failure; collect failures into a summary table
#   - Exit 0 only if every job passed; exit 1 if any job failed
#   - Per-job stdout/stderr captured to:
#       tmp/integration_testing/run_all_<timestamp>/<job>.log
# =============================================================================

set -uo pipefail

# ── Colours ─────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# ── Paths ───────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
SHELL_COMPAT="${SCRIPT_DIR}/../utils/shell_compat.sh"
DEADLINE_RUNNER="${SCRIPT_DIR}/../utils/run_with_deadline.py"
PREPARE_SCRIPT="${SCRIPT_DIR}/../utils/prepare.sh"
# shellcheck source=../utils/shell_compat.sh
source "$SHELL_COMPAT"
TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
WORK_DIR="${PROJECT_ROOT}/tmp/integration_testing/run_all_${TIMESTAMP}"

# ── Job definitions ─────────────────────────────────────────────────────────
# Names are stable identifiers used by --skip / --only and the report. The
# command is resolved immediately before launch so runtime flags are included.
JOB_NAMES=(
    "main"
    "linksys_etcd"
    "linksys_es"
    "linksys_otlp"
    "linksys_kafka"
    "acme"
    "cli"
    "file_watcher"
)

job_command() {
    case "$1" in
        main)         echo "${SCRIPT_DIR}/run_integration.sh --no-prepare ${MAIN_FULL_FLAG}" ;;
        linksys_etcd) echo "${SCRIPT_DIR}/link_sys/run_etcd_test.sh --no-build" ;;
        linksys_es)   echo "${SCRIPT_DIR}/link_sys/run_es_test.sh --no-build" ;;
        linksys_otlp)  echo "${SCRIPT_DIR}/link_sys/run_otlp_test.sh --no-build" ;;
        linksys_kafka) echo "${SCRIPT_DIR}/link_sys/run_kafka_test.sh --no-build" ;;
        acme)         echo "${SCRIPT_DIR}/run_acme_test.sh --no-build" ;;
        cli)          echo "${SCRIPT_DIR}/test_cli_commands.sh" ;;
        file_watcher) echo "${SCRIPT_DIR}/test_file_watcher.sh" ;;
        *)            return 1 ;;
    esac
}

# ── Args ────────────────────────────────────────────────────────────────────
DO_PREPARE=true
FULL_TEST=false
SKIP_LIST=""
ONLY_LIST=""
JOB_TIMEOUT_SECONDS="${EDGION_RUN_ALL_JOB_TIMEOUT_SECONDS:-3600}"
RUN_TIMEOUT_SECONDS="${EDGION_RUN_ALL_TIMEOUT_SECONDS:-21600}"
DEADLINE_GRACE_SECONDS="${EDGION_DEADLINE_GRACE_SECONDS:-10}"

show_help() {
    sed -n '2,27p' "${BASH_SOURCE[0]}"
    echo ""
    echo "Available job names:"
    for name in "${JOB_NAMES[@]}"; do
        echo "  - ${name}"
    done
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-prepare) DO_PREPARE=false; shift ;;
        --full-test)  FULL_TEST=true; shift ;;
        --skip)
            if [ $# -lt 2 ]; then echo "Missing argument for --skip" >&2; exit 2; fi
            SKIP_LIST="$2"; shift 2 ;;
        --only)
            if [ $# -lt 2 ]; then echo "Missing argument for --only" >&2; exit 2; fi
            ONLY_LIST="$2"; shift 2 ;;
        --job-timeout-secs)
            if [ $# -lt 2 ]; then echo "Missing argument for --job-timeout-secs" >&2; exit 2; fi
            JOB_TIMEOUT_SECONDS="$2"; shift 2 ;;
        --run-timeout-secs)
            if [ $# -lt 2 ]; then echo "Missing argument for --run-timeout-secs" >&2; exit 2; fi
            RUN_TIMEOUT_SECONDS="$2"; shift 2 ;;
        -h|--help) show_help; exit 0 ;;
        *) echo "Unknown option: $1" >&2; show_help >&2; exit 2 ;;
    esac
done

if [ -n "$SKIP_LIST" ] && [ -n "$ONLY_LIST" ]; then
    echo "Error: --skip and --only are mutually exclusive" >&2
    exit 2
fi

validate_positive_seconds() {
    local label="$1" value="$2"
    case "$value" in
        ''|*[!0-9]*)
            echo "Error: ${label} must be a positive integer" >&2
            exit 2
            ;;
    esac
    if [ "$value" -lt 1 ]; then
        echo "Error: ${label} must be at least 1" >&2
        exit 2
    fi
}
validate_positive_seconds "job timeout" "$JOB_TIMEOUT_SECONDS"
validate_positive_seconds "run timeout" "$RUN_TIMEOUT_SECONDS"
validate_positive_seconds "deadline grace" "$DEADLINE_GRACE_SECONDS"
if [ ! -f "$DEADLINE_RUNNER" ]; then
    echo "Error: deadline runner not found: ${DEADLINE_RUNNER}" >&2
    exit 2
fi

if $FULL_TEST; then
    MAIN_FULL_FLAG="--full-test"
else
    MAIN_FULL_FLAG=""
fi

# ── Validate --only / --skip names against JOB_NAMES ───────────────────────
is_known_job() {
    local needle="$1"
    for n in "${JOB_NAMES[@]}"; do
        [ "$n" = "$needle" ] && return 0
    done
    return 1
}

validate_name_list() {
    local label="$1" csv="$2"
    [ -z "$csv" ] && return 0
    local IFS=','
    for n in $csv; do
        if ! is_known_job "$n"; then
            echo "Error: unknown job name '$n' in --${label}" >&2
            echo "Available: ${JOB_NAMES[*]}" >&2
            exit 2
        fi
    done
}
validate_name_list skip "$SKIP_LIST"
validate_name_list only "$ONLY_LIST"

job_selected() {
    local name="$1"
    if [ -n "$ONLY_LIST" ]; then
        case ",$ONLY_LIST," in *",$name,"*) return 0 ;; *) return 1 ;; esac
    fi
    if [ -n "$SKIP_LIST" ]; then
        case ",$SKIP_LIST," in *",$name,"*) return 1 ;; *) return 0 ;; esac
    fi
    return 0
}

# ── Helpers ─────────────────────────────────────────────────────────────────
log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_error()   { echo -e "${RED}[✗]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[!]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}  $1${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

human_duration() {
    local secs=$1
    if [ "$secs" -lt 60 ]; then
        printf '%ds' "$secs"
    elif [ "$secs" -lt 3600 ]; then
        printf '%dm%02ds' $((secs / 60)) $((secs % 60))
    else
        printf '%dh%02dm%02ds' $((secs / 3600)) $(((secs % 3600) / 60)) $((secs % 60))
    fi
}

OVERALL_START=$(date +%s)

remaining_run_seconds() {
    local now elapsed
    now=$(date +%s)
    elapsed=$((now - OVERALL_START))
    echo $((RUN_TIMEOUT_SECONDS - elapsed))
}

bounded_job_timeout() {
    local remaining
    remaining=$(remaining_run_seconds)
    if [ "$remaining" -lt "$JOB_TIMEOUT_SECONDS" ]; then
        echo "$remaining"
    else
        echo "$JOB_TIMEOUT_SECONDS"
    fi
}

CURRENT_WATCHDOG_PID=""
forward_termination() {
    local signal_name="$1" exit_code=143
    [ "$signal_name" = "INT" ] && exit_code=130
    if [ -n "$CURRENT_WATCHDOG_PID" ] && kill -0 "$CURRENT_WATCHDOG_PID" 2>/dev/null; then
        kill -"$signal_name" "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
        wait "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
    fi
    exit "$exit_code"
}
trap 'forward_termination INT' INT
trap 'forward_termination TERM' TERM

declare -a RESULT_NAMES=()
declare -a RESULT_STATUS=()
declare -a RESULT_DURATION=()
declare -a RESULT_LOG=()
BUILD_BLOCKED=false
BUILD_BLOCKED_STATUS="NOT_STARTED"

# ── Top-level build (optional, single pass) ────────────────────────────────
mkdir -p "$WORK_DIR"
log_info "Run-all work directory: $WORK_DIR"

if $DO_PREPARE; then
    log_section "Build: prepare integration artifacts (top-level, once)"
    build_start=$(date +%s)
    build_timeout=$(bounded_job_timeout)
    if [ "$build_timeout" -lt 1 ]; then
        echo "Whole-run deadline reached before build" > "${WORK_DIR}/build.log"
        build_rc=124
    else
        python3 "$DEADLINE_RUNNER" --timeout-secs "$build_timeout" --grace-secs "$DEADLINE_GRACE_SECONDS" -- \
            "$PREPARE_SCRIPT" \
            > "${WORK_DIR}/build.log" 2>&1 &
        CURRENT_WATCHDOG_PID=$!
        set +e
        wait "$CURRENT_WATCHDOG_PID"
        build_rc=$?
        set -e
        CURRENT_WATCHDOG_PID=""
        tail -5 "${WORK_DIR}/build.log"
    fi
    build_duration=$(( $(date +%s) - build_start ))
    if [ "$build_rc" -eq 0 ]; then
        log_success "Build complete"
    elif [ "$build_rc" -eq 124 ]; then
        log_error "Top-level build timed out after ${build_timeout}s — see ${WORK_DIR}/build.log"
        RESULT_NAMES+=("build")
        RESULT_STATUS+=("TIMEOUT")
        RESULT_DURATION+=("$build_duration")
        RESULT_LOG+=("${WORK_DIR}/build.log")
        BUILD_BLOCKED=true
        BUILD_BLOCKED_STATUS="TIMEOUT"
    else
        log_error "Top-level build failed — see ${WORK_DIR}/build.log"
        RESULT_NAMES+=("build")
        RESULT_STATUS+=("FAIL")
        RESULT_DURATION+=("$build_duration")
        RESULT_LOG+=("${WORK_DIR}/build.log")
        BUILD_BLOCKED=true
    fi
else
    log_info "Skipping top-level build (--no-prepare); required artifacts must already exist."
fi

# ── Run jobs ────────────────────────────────────────────────────────────────
run_job() {
    local name="$1"
    local cmd
    cmd="$(job_command "$name")"
    local log_file="${WORK_DIR}/${name}.log"

    log_section "Job: ${name}"
    log_info "Command: ${cmd}"
    log_info "Log:     ${log_file}"

    local start_ts end_ts
    start_ts=$(date +%s)

    local timeout_seconds
    timeout_seconds=$(bounded_job_timeout)
    if [ "$timeout_seconds" -lt 1 ]; then
        RESULT_NAMES+=("$name")
        RESULT_DURATION+=("0")
        RESULT_LOG+=("$log_file")
        RESULT_STATUS+=("TIMEOUT")
        echo "Whole-run deadline reached before job start" > "$log_file"
        log_error "${name} not started: whole-run deadline reached"
        return 0
    fi

    python3 "$DEADLINE_RUNNER" \
        --timeout-secs "$timeout_seconds" \
        --grace-secs "$DEADLINE_GRACE_SECONDS" \
        -- bash -c "$cmd" > "$log_file" 2>&1 &
    CURRENT_WATCHDOG_PID=$!
    set +e
    wait "$CURRENT_WATCHDOG_PID"
    local rc=$?
    set -e
    CURRENT_WATCHDOG_PID=""

    end_ts=$(date +%s)
    local duration=$((end_ts - start_ts))

    RESULT_NAMES+=("$name")
    RESULT_DURATION+=("$duration")
    RESULT_LOG+=("$log_file")
    if [ $rc -eq 0 ]; then
        RESULT_STATUS+=("PASS")
        log_success "${name} passed in $(human_duration "$duration")"
    elif [ $rc -eq 124 ]; then
        RESULT_STATUS+=("TIMEOUT")
        log_error "${name} timed out after $(human_duration "$timeout_seconds") — see ${log_file}"
        log_warn "Continuing with remaining jobs..."
    else
        RESULT_STATUS+=("FAIL")
        log_error "${name} failed (rc=${rc}) in $(human_duration "$duration") — see ${log_file}"
        log_warn "Continuing with remaining jobs..."
    fi
}

for name in "${JOB_NAMES[@]}"; do
    if job_selected "$name"; then
        if $BUILD_BLOCKED; then
            blocked_log="${WORK_DIR}/${name}.log"
            echo "Not started because the top-level build did not complete" > "$blocked_log"
            RESULT_NAMES+=("$name")
            RESULT_STATUS+=("$BUILD_BLOCKED_STATUS")
            RESULT_DURATION+=("0")
            RESULT_LOG+=("$blocked_log")
            log_error "${name} not started because the top-level build did not complete"
        else
            run_job "$name"
        fi
    else
        log_info "Skipping job: ${name}"
    fi
done
OVERALL_END=$(date +%s)
OVERALL_DURATION=$((OVERALL_END - OVERALL_START))

# ── Report ──────────────────────────────────────────────────────────────────
log_section "Integration Suite Report"

passed=0
failed=0
timed_out=0
for i in "${!RESULT_NAMES[@]}"; do
    if [ "${RESULT_STATUS[$i]}" = "PASS" ]; then
        passed=$((passed + 1))
    else
        failed=$((failed + 1))
        if [ "${RESULT_STATUS[$i]}" = "TIMEOUT" ]; then
            timed_out=$((timed_out + 1))
        fi
    fi
done

# Header
printf "%-8s %-16s %10s   %s\n" "STATUS" "JOB" "DURATION" "LOG"
printf '%s\n' "------------------------------------------------------------------------"
for i in "${!RESULT_NAMES[@]}"; do
    local_status="${RESULT_STATUS[$i]}"
    local_color=""
    if [ "$local_status" = "PASS" ]; then
        local_color="$GREEN"
    else
        local_color="$RED"
    fi
    printf "${local_color}%-8s${NC} %-16s %10s   %s\n" \
        "[$local_status]" \
        "${RESULT_NAMES[$i]}" \
        "$(human_duration "${RESULT_DURATION[$i]}")" \
        "${RESULT_LOG[$i]}"
done
printf '%s\n' "------------------------------------------------------------------------"

ran=$((passed + failed))
echo "Ran: ${ran}    Passed: ${passed}    Failed: ${failed}    Timed out: ${timed_out}    Total time: $(human_duration "$OVERALL_DURATION")"

# Save machine-readable summary
{
    echo "timestamp=${TIMESTAMP}"
    echo "total_seconds=${OVERALL_DURATION}"
    echo "ran=${ran}"
    echo "passed=${passed}"
    echo "failed=${failed}"
    echo "timed_out=${timed_out}"
    for i in "${!RESULT_NAMES[@]}"; do
        echo "${RESULT_NAMES[$i]}=${RESULT_STATUS[$i]}:${RESULT_DURATION[$i]}"
    done
} > "${WORK_DIR}/summary.txt"

if [ $failed -gt 0 ]; then
    log_error "Some integration jobs failed — see logs above"
    exit 1
fi

log_success "All integration jobs passed"
exit 0
