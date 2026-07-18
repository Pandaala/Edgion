#!/usr/bin/env bash
# Shared macOS Bash 3.2 / Linux Bash 5 compatibility helpers.
#
# Standard integration entry points source this file and export it through
# BASH_ENV, so descendant Bash scripts receive the same helpers automatically.

if [ "${EDGION_SHELL_COMPAT_LOADED:-}" = "1" ]; then
    return 0 2>/dev/null || exit 0
fi
EDGION_SHELL_COMPAT_LOADED=1

EDGION_SHELL_COMPAT_FILE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
case "$(uname -s)" in
    Darwin) EDGION_SHELL_PLATFORM="darwin" ;;
    Linux)  EDGION_SHELL_PLATFORM="linux" ;;
    *)
        echo "shell_compat: unsupported operating system: $(uname -s)" >&2
        return 1 2>/dev/null || exit 1
        ;;
esac
export EDGION_SHELL_COMPAT_FILE EDGION_SHELL_PLATFORM
export BASH_ENV="$EDGION_SHELL_COMPAT_FILE"

# Dispatch an operation to its platform implementation. Platform branching
# stays in this file; callers use only the stable public helpers below.
edgion_platform_call() {
    local operation="$1" implementation
    shift
    implementation="_edgion_${operation}_${EDGION_SHELL_PLATFORM}"
    if ! declare -F "$implementation" >/dev/null 2>&1; then
        echo "shell_compat: operation '${operation}' is not implemented for ${EDGION_SHELL_PLATFORM}" >&2
        return 2
    fi
    "$implementation" "$@"
}

# Count non-empty input lines and always return success, including for zero.
# This avoids grep -c returning status 1 before a set -e caller can assert on 0.
edgion_count_nonempty_lines() {
    awk 'NF { count++ } END { print count + 0 }'
}

# Read an exact JSON integer from stdin without jq's IEEE-754 rounding.
# Usage: printf '%s' "$json" | edgion_json_integer DEFAULT key [nested_key ...]
edgion_json_integer() {
    local default_value="$1"
    shift
    python3 -c '
import json
import sys

default = sys.argv[1]
value = json.load(sys.stdin)
for key in sys.argv[2:]:
    if not isinstance(value, dict) or key not in value:
        print(default)
        raise SystemExit(0)
    value = value[key]
if isinstance(value, bool) or not isinstance(value, int):
    print(default)
else:
    print(value)
' "$default_value" "$@"
}

# Print live, non-zombie PIDs owned by the current uid for an exact process
# name. Empty output is a normal success result.
_edgion_live_pids_common() {
    local process_name="$1" candidate_pids pid state
    candidate_pids=$(pgrep -u "$(id -u)" "$process_name" 2>/dev/null || true)
    for pid in $candidate_pids; do
        state=$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ' || true)
        case "$state" in
            ""|Z*) continue ;;
        esac
        printf '%s\n' "$pid"
    done
    return 0
}

_edgion_live_pids_darwin() { _edgion_live_pids_common "$@"; }
_edgion_live_pids_linux()  { _edgion_live_pids_common "$@"; }

edgion_live_pids_by_name() {
    edgion_platform_call live_pids "$@"
}

# Print required loopback IPs that are not available. Linux routes 127/8 to
# loopback by default; Darwin requires explicit lo0 aliases.
_edgion_missing_loopback_aliases_linux() {
    return 0
}

_edgion_missing_loopback_aliases_darwin() {
    local lo_inet ip
    lo_inet=$(ifconfig lo0 2>/dev/null | awk '/inet /{print $2}')
    for ip in "$@"; do
        if ! printf '%s\n' "$lo_inet" | grep -qx "$ip"; then
            printf '%s\n' "$ip"
        fi
    done
}

edgion_missing_loopback_aliases() {
    edgion_platform_call missing_loopback_aliases "$@"
}

# Run a command or shell function once per interval until it succeeds or the
# integer timeout is reached. The first attempt is immediate.
# Usage: edgion_wait_until TIMEOUT_SECONDS INTERVAL_SECONDS command [args ...]
edgion_wait_until() {
    local timeout_seconds="$1" interval_seconds="$2" elapsed=0
    shift 2
    while [ "$elapsed" -le "$timeout_seconds" ]; do
        if "$@"; then
            return 0
        fi
        if [ "$elapsed" -eq "$timeout_seconds" ]; then
            break
        fi
        sleep "$interval_seconds"
        elapsed=$((elapsed + interval_seconds))
    done
    return 1
}
