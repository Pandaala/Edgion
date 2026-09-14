#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
KILL_SCRIPT="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils/kill_all.sh"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

bash -n "$KILL_SCRIPT"

wait_function=$(sed -n '/^shutdown_wait_seconds()/,/^}/p' "$KILL_SCRIPT")
force_kill_function=$(sed -n '/^force_kill()/,/^}/p' "$KILL_SCRIPT")
[ -n "$wait_function" ] || fail "shutdown wait selector not found"
[ -n "$force_kill_function" ] || fail "force_kill function not found"
eval "$wait_function"
eval "$force_kill_function"

[ "$(EDGION_COVERAGE=1 shutdown_wait_seconds edgion-gateway)" = "60" ] \
    || fail "coverage Gateway does not receive the profile-flush window"
[ "$(EDGION_COVERAGE=0 shutdown_wait_seconds edgion-gateway)" = "5" ] \
    || fail "ordinary Gateway teardown no longer uses the short window"
[ "$(EDGION_COVERAGE=1 shutdown_wait_seconds edgion-controller)" = "5" ] \
    || fail "coverage widened a non-Gateway teardown window"

test_tmp=$(mktemp -d)
fixture_pid=""
cleanup() {
    if [ -n "$fixture_pid" ]; then
        kill -KILL "$fixture_pid" 2>/dev/null || true
        wait "$fixture_pid" 2>/dev/null || true
    fi
    rm -rf "$test_tmp"
}
trap cleanup EXIT

fixture="${test_tmp}/profile-flush-fixture.sh"
cat > "$fixture" <<'EOF'
#!/bin/bash
marker=$1
delay=$2
ready=$3
trap 'sleep "$delay"; touch "$marker"; exit 0' TERM
touch "$ready"
while true; do
    sleep 1
done
EOF
chmod 755 "$fixture"

log_info() { :; }

wait_for_fixture() {
    local ready=$1
    local waited=0
    while [ "$waited" -lt 20 ] && [ ! -f "$ready" ]; do
        sleep 0.1
        waited=$((waited + 1))
    done
    [ -f "$ready" ] || fail "profile-flush fixture did not become ready"
}

graceful_marker="${test_tmp}/graceful-profile"
graceful_ready="${test_tmp}/graceful-ready"
"$fixture" "$graceful_marker" 1 "$graceful_ready" &
fixture_pid=$!
wait_for_fixture "$graceful_ready"
force_kill "$fixture" "edgion-gateway" 3
wait "$fixture_pid" 2>/dev/null || true
fixture_pid=""
[ -f "$graceful_marker" ] \
    || fail "bounded graceful teardown did not run the profile-flush hook"

killed_marker="${test_tmp}/killed-profile"
killed_ready="${test_tmp}/killed-ready"
"$fixture" "$killed_marker" 3 "$killed_ready" &
fixture_pid=$!
wait_for_fixture "$killed_ready"
force_kill "$fixture" "edgion-gateway" 1
wait "$fixture_pid" 2>/dev/null || true
fixture_pid=""
[ ! -f "$killed_marker" ] \
    || fail "forced teardown unexpectedly completed the profile-flush hook"

echo "OK: integration coverage teardown boundaries passed"
