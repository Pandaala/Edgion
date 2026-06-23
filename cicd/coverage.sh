#!/usr/bin/env bash
# Coverage orchestration via cargo-llvm-cov. Run directly from anywhere in the
# workspace.
#
# Usage: cicd/coverage.sh <full|unit|integration|report|clean>
#   full         clean + unit + integration + report (default)
#   unit         unit tests only, collect profraw
#   integration  integration tests with instrumented binaries
#   report       merge profraw and emit HTML + lcov
#   clean        remove profraw and coverage reports
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$REPO_ROOT"

IGNORE_REGEX='(\.c|\.cc|\.cpp|vendor/|\.generated\.)'

cov_clean() {
    cargo llvm-cov clean --workspace
    rm -f target/*.profraw
    rm -f target/llvm-cov-target/*.profraw
    rm -f target/llvm-cov-target/Edgion-profraw-list target/llvm-cov-target/Edgion.profdata
    rm -rf target/coverage
}

cov_unit() {
    cargo llvm-cov --no-report --workspace
}

cov_integration() {
    EDGION_COVERAGE=1 ./edgion-tests/integration/scripts/integration/run_integration.sh
}

cov_report() {
    cargo llvm-cov report \
        --html --output-dir target/coverage \
        --ignore-filename-regex "$IGNORE_REGEX"
    cargo llvm-cov report \
        --lcov --output-path target/coverage/lcov.info \
        --ignore-filename-regex "$IGNORE_REGEX"
    echo "Report: target/coverage/html/index.html"
}

case "${1:-full}" in
    full)        cov_clean; cov_unit; cov_integration; cov_report ;;
    unit)        cov_unit ;;
    integration) cov_integration ;;
    report)      cov_report ;;
    clean)       cov_clean ;;
    *) echo "Usage: $0 <full|unit|integration|report|clean>" >&2; exit 1 ;;
esac
