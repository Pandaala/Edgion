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
#   run_all_integration.sh --help
#
# Behaviour:
#   - Continue on failure; collect failures into a summary table
#   - Exit 0 only if every job passed; exit 1 if any job failed
#   - Per-job stdout/stderr captured to:
#       integration_testing/run_all_<timestamp>/<job>.log
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
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
WORK_DIR="${PROJECT_ROOT}/integration_testing/run_all_${TIMESTAMP}"

# ── Job definitions ─────────────────────────────────────────────────────────
# Each job is "<name>|<command>". Names are stable identifiers used by
# --skip / --only and the report. Commands are evaluated with `eval` so they
# can reference variables (e.g. $MAIN_FULL_FLAG) computed at runtime.
JOB_NAMES=(
    "main"
    "linksys_etcd"
    "linksys_es"
    "acme"
    "cli"
    "file_watcher"
)

job_command() {
    case "$1" in
        main)         echo "${SCRIPT_DIR}/run_integration.sh --no-prepare ${MAIN_FULL_FLAG}" ;;
        linksys_etcd) echo "${SCRIPT_DIR}/link_sys/run_etcd_test.sh --no-build" ;;
        linksys_es)   echo "${SCRIPT_DIR}/link_sys/run_es_test.sh --no-build" ;;
        acme)         echo "${SCRIPT_DIR}/run_acme_test.sh" ;;
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
        -h|--help) show_help; exit 0 ;;
        *) echo "Unknown option: $1" >&2; show_help >&2; exit 2 ;;
    esac
done

if [ -n "$SKIP_LIST" ] && [ -n "$ONLY_LIST" ]; then
    echo "Error: --skip and --only are mutually exclusive" >&2
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

# ── Top-level build (optional, single pass) ────────────────────────────────
mkdir -p "$WORK_DIR"
log_info "Run-all work directory: $WORK_DIR"

if $DO_PREPARE; then
    log_section "Build: cargo build (top-level, once)"
    if (cd "$PROJECT_ROOT" && cargo build \
        -p edgion-controller --bin edgion-controller \
        -p edgion-gateway --bin edgion-gateway \
        -p edgion-cli --bin edgion-cli \
        -p edgion-tests --bin test_server) \
        2>&1 | tee "${WORK_DIR}/build.log" | tail -5; then
        log_success "Build complete"
    else
        log_error "Top-level build failed — see ${WORK_DIR}/build.log"
        exit 1
    fi
else
    log_info "Skipping top-level build (--no-prepare). Children may still rebuild incrementally."
fi

# ── Run jobs ────────────────────────────────────────────────────────────────
declare -a RESULT_NAMES=()
declare -a RESULT_STATUS=()
declare -a RESULT_DURATION=()
declare -a RESULT_LOG=()

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

    set +e
    eval "$cmd" > "$log_file" 2>&1
    local rc=$?
    set -e

    end_ts=$(date +%s)
    local duration=$((end_ts - start_ts))

    RESULT_NAMES+=("$name")
    RESULT_DURATION+=("$duration")
    RESULT_LOG+=("$log_file")
    if [ $rc -eq 0 ]; then
        RESULT_STATUS+=("PASS")
        log_success "${name} passed in $(human_duration "$duration")"
    else
        RESULT_STATUS+=("FAIL")
        log_error "${name} failed (rc=${rc}) in $(human_duration "$duration") — see ${log_file}"
        log_warn "Continuing with remaining jobs..."
    fi
}

OVERALL_START=$(date +%s)
for name in "${JOB_NAMES[@]}"; do
    if job_selected "$name"; then
        run_job "$name"
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
for i in "${!RESULT_NAMES[@]}"; do
    if [ "${RESULT_STATUS[$i]}" = "PASS" ]; then
        passed=$((passed + 1))
    else
        failed=$((failed + 1))
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
echo "Ran: ${ran}    Passed: ${passed}    Failed: ${failed}    Total time: $(human_duration "$OVERALL_DURATION")"

# Save machine-readable summary
{
    echo "timestamp=${TIMESTAMP}"
    echo "total_seconds=${OVERALL_DURATION}"
    echo "ran=${ran}"
    echo "passed=${passed}"
    echo "failed=${failed}"
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
