#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../../edgion-tests/integration/scripts/utils/shell_compat.sh
source "${SCRIPT_DIR}/../../../edgion-tests/integration/scripts/utils/shell_compat.sh"

case "$EDGION_SHELL_PLATFORM" in
    darwin|linux) ;;
    *) echo "ERROR: unexpected platform adapter: $EDGION_SHELL_PLATFORM" >&2; exit 1 ;;
esac

_edgion_test_dispatch_darwin() { printf 'darwin\n'; }
_edgion_test_dispatch_linux() { printf 'linux\n'; }
[ "$(edgion_platform_call test_dispatch)" = "$EDGION_SHELL_PLATFORM" ]

[ "$(printf '' | edgion_count_nonempty_lines)" = "0" ]
[ "$(printf 'one\n\ntwo\n' | edgion_count_nonempty_lines)" = "2" ]

large_integer="9223372036854775807"
parsed_integer=$(printf '{"data":{"id":%s}}' "$large_integer" | edgion_json_integer 0 data id)
[ "$parsed_integer" = "$large_integer" ]
[ "$(printf '{}' | edgion_json_integer 7 data id)" = "7" ]

attempts=0
succeed_on_third_attempt() {
    attempts=$((attempts + 1))
    [ "$attempts" -ge 3 ]
}
edgion_wait_until 4 1 succeed_on_third_attempt
if edgion_wait_until 0 1 false; then
    echo "ERROR: wait helper accepted a command that never succeeded" >&2
    exit 1
fi

sleep 10 &
sleep_pid=$!
cleanup() {
    kill "$sleep_pid" 2>/dev/null || true
    wait "$sleep_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
if ! edgion_live_pids_by_name sleep | grep -qx "$sleep_pid"; then
    echo "ERROR: live-process helper did not return the test process" >&2
    exit 1
fi

[ "$BASH_ENV" = "$EDGION_SHELL_COMPAT_FILE" ]
if ! bash -c 'command -v edgion_platform_call >/dev/null && command -v edgion_wait_until >/dev/null && command -v edgion_json_integer >/dev/null'; then
    echo "ERROR: BASH_ENV did not bootstrap helpers in a child Bash process" >&2
    exit 1
fi
echo "OK: shared shell compatibility helpers passed"
