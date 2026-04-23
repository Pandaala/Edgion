#!/usr/bin/env bash
# verify_log_only_diff.sh
#
# Heuristic triage for the plugin-log-overhaul. Given a git diff range,
# output every added/removed line in plugin .rs files that does NOT
# match a known-safe log-only pattern. Output goes to stdout. Exit code
# is always 0; the reviewer decides what to do with the output.
#
# Usage:
#   scripts/verify_log_only_diff.sh <git-diff-range>
#   scripts/verify_log_only_diff.sh log-overhaul-start..HEAD
#
# Safe patterns for a `+` or `-` line (heuristic):
#   - Line containing a push / push_err / tracing::*! call with string literal
#   - Comment-only line (// or ///)
#   - Integral tracing::(error|warn|info|debug|trace)! line (add or delete)
#   - `use` statement change
#   - Whitespace-only change

set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <git-diff-range>" >&2
  exit 2
fi

RANGE="$1"
PATHS=("src/core/gateway/plugins/")

git diff --unified=0 "$RANGE" -- "${PATHS[@]}" | awk '
  /^\+\+\+ b\// { file = substr($0, 7); next }
  /^[+-]/ && !/^(\+\+\+|---)/ {
    line = substr($0, 2)
    if (line ~ /^[[:space:]]*$/) next
    if (line ~ /^[[:space:]]*\/\//) next
    if (line ~ /^[[:space:]]*use[[:space:]]/) next
    if (line ~ /tracing::(error|warn|info|debug|trace)!/) next
    if (line ~ /\.push(_err)?\([[:space:]]*"/) next
    printf "%s:?: %s%s\n", file, substr($0,1,1), line
  }
'
