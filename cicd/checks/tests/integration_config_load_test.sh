#!/bin/bash
# Exercise the production loader without starting services or using the network.
set -eu

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
START="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils/start_all_with_conf.sh"
test_tmp=$(mktemp -d)
trap 'rm -rf "$test_tmp"' EXIT
SCRIPT_DIR="$test_tmp"
LOG_DIR="$test_tmp"

fail() { echo "FAIL: $*" >&2; exit 1; }
log_section() { :; }
log_info() { :; }
log_warning() { :; }
log_success() { echo "SUCCESS: $*"; }
log_error() { echo "ERROR: $*"; }
get_suites_to_load() { echo "EdgionPlugins/Csrf EdgionPlugins/Cors"; }

loader=$(sed -n '/^load_configs()/,/^}/p' "$START")
[ -n "$loader" ] || fail "production load_configs function not found"
eval "$loader"
cat > "${test_tmp}/load_conf.sh" <<'EOF'
echo "Applying $3/fixture.yaml"
if [ "$3" = "EdgionPlugins/Csrf" ]; then
    exit "$LOADER_STATUS"
fi
EOF

export LOADER_STATUS=0
load_configs > "${test_tmp}/success.out" || fail "successful loading rejected"
grep -q 'allconfigLoadcompleted' "${test_tmp}/success.out" || fail "success not reported"
grep -q 'EdgionPlugins/Cors/fixture.yaml' "${test_tmp}/load_config.log" || fail "suite output not logged"

export LOADER_STATUS=7
if load_configs > "${test_tmp}/failure.out"; then
    fail "loader failure hidden by successful tee"
fi
grep -q 'failed for EdgionPlugins/Csrf' "${test_tmp}/failure.out" || fail "failed suite not identified"
grep -q 'load_config.log' "${test_tmp}/failure.out" || fail "diagnostic omits log path"
grep -q 'SUCCESS: EdgionPlugins/Cors' "${test_tmp}/failure.out" || fail "remaining suite was not attempted"
if grep -q 'allconfigLoadcompleted' "${test_tmp}/failure.out"; then
    fail "partial failure reported as complete success"
fi

# A log sink failure is independently fatal even when every loader succeeds.
export LOADER_STATUS=0
tee() { cat >/dev/null; return 9; }
if load_configs > "${test_tmp}/sink-failure.out"; then
    fail "log sink failure accepted"
fi
unset -f tee
if set -o | grep '^pipefail' | grep -q 'on'; then
    fail "loader changed caller shell options"
fi
echo "PASS: config load success, loader failure, log sink failure, and shell option isolation"

# Exercise real suite discovery: standalone OTLP listeners must never enter the
# default shared stack, where port 31932 belongs to HTTPRoute/Delegation.
discovery=$(sed -n '/^get_suites_to_load()/,/^}/p' "$START")
[ -n "$discovery" ] || fail "production get_suites_to_load function not found"
eval "$discovery"
suite_is_supported() { return 0; }
SUITES=""
default_suites=" $(get_suites_to_load) "
case "$default_suites" in
    *" LinkSys/Otlp "*|*" LinkSys/Otlp/"*) fail "standalone OTLP config entered the shared stack" ;;
esac
for shared_suite in HTTPRoute/Delegation LinkSys/Redis LinkSys/Status; do
    case "$default_suites" in
        *" $shared_suite "*) ;;
        *) fail "shared suite disappeared: $shared_suite" ;;
    esac
done
SUITES="LinkSys/Otlp"
[ "$(get_suites_to_load)" = " LinkSys/Otlp" ] || fail "explicit OTLP selection was lost"
echo "PASS: default shared-suite isolation and explicit OTLP selection"
