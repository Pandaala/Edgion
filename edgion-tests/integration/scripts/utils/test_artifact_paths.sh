#!/bin/bash
# No services/builds. Negative ownership probe may signal only its own sleep child.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
source "$SCRIPT_DIR/artifact_paths.sh"
ps() { printf '%s/target/debug/test_server --fixture\n' "$PROJECT_ROOT"; }
if edgion_refuse_existing_topology; then echo 'Existing topology was accepted' >&2; exit 1; fi
unset -f ps
EDGION_COVERAGE=0
EDGION_TEST_PROFILE=debug
edgion_resolve_artifacts
[ "$EDGION_TEST_BIN_DIR" = "$PROJECT_ROOT/target/debug" ]
EDGION_TEST_PROFILE=release
edgion_resolve_artifacts
[ "$EDGION_TEST_BIN_DIR" = "$PROJECT_ROOT/target/release" ]
EDGION_COVERAGE=1
if edgion_resolve_artifacts; then echo 'Release coverage was accepted' >&2; exit 1; fi
EDGION_TEST_PROFILE=debug
edgion_resolve_artifacts
[ "$EDGION_TEST_BIN_DIR" = "$PROJECT_ROOT/target/llvm-cov-target/debug" ]
EDGION_TEST_PROFILE=unexpected
if edgion_resolve_artifacts; then echo 'Unknown profile was accepted' >&2; exit 1; fi
EDGION_COVERAGE=0
EDGION_TEST_PROFILE=release
export EDGION_TEST_PROFILE EDGION_COVERAGE
if bash "$SCRIPT_DIR/start_all.sh" >/dev/null 2>&1; then
    echo 'Legacy release startup was accepted' >&2; exit 1
fi
mkdir -p "$PROJECT_ROOT/tmp/integration_testing"
EDGION_TEST_OWNED_WORK_DIR=$(mktemp -d "$PROJECT_ROOT/tmp/integration_testing/ai-performance.XXXXXX")
export EDGION_TEST_OWNED_WORK_DIR
child=""
cleanup_probe() {
    if [ -n "$child" ]; then kill "$child" 2>/dev/null || true; wait "$child" 2>/dev/null || true; fi
    rm -rf "$EDGION_TEST_OWNED_WORK_DIR"
}
trap cleanup_probe EXIT
edgion_validate_owned_work_dir "$EDGION_TEST_OWNED_WORK_DIR"
if edgion_validate_owned_work_dir "$PROJECT_ROOT"; then exit 1; fi
# Before the first spawn, absent provenance means there is nothing owned to stop.
bash "$SCRIPT_DIR/kill_all.sh"
printf 'release\n' > "$EDGION_TEST_OWNED_WORK_DIR/artifact-profile"
mkdir "$EDGION_TEST_OWNED_WORK_DIR/pids"
sleep 30 &
child=$!
printf '%s\n' "$child" > "$EDGION_TEST_OWNED_WORK_DIR/pids/gateway.pid"
if bash "$SCRIPT_DIR/kill_all.sh"; then
    echo 'Changed executable ownership was accepted' >&2; exit 1
fi
kill -0 "$child"
EDGION_TEST_OWNED_WORK_DIR='' bash "$SCRIPT_DIR/kill_all.sh" >/dev/null 2>&1 && exit 1
# Exercise the certificate generator's actual CLI dispatch without crypto or services.
probe_repo="$EDGION_TEST_OWNED_WORK_DIR/cert-generator-probe"
mkdir -p "$probe_repo/edgion-tests/integration/scripts/gen_certs" \
    "$probe_repo/edgion-tests/integration/scripts/utils"
cp "$SCRIPT_DIR/../gen_certs/generate_conf_sync_certs.sh" \
    "$probe_repo/edgion-tests/integration/scripts/gen_certs/"
cp "$SCRIPT_DIR/artifact_paths.sh" "$probe_repo/edgion-tests/integration/scripts/utils/"
mock_cli="$probe_repo/mock-cli"
cat > "$mock_cli" <<'MOCK'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$0" >> "$EDGION_CLI_TEST_RECORD"
output_dir="" cert_name=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output-dir) output_dir="$2"; shift 2 ;;
        --cert-name) cert_name="$2"; shift 2 ;;
        *) shift ;;
    esac
done
[ -n "$output_dir" ] && [ -n "$cert_name" ]
mkdir -p "$output_dir/$cert_name"
for file in ca.crt server.crt server.key client.crt client.key; do
    printf 'fixture\n' > "$output_dir/$cert_name/$file"
done
MOCK
chmod 755 "$mock_cli"
for case_name in debug release coverage override; do
    profile=debug coverage=0 override=""
    case "$case_name" in
        release) profile=release; bin_dir="$probe_repo/target/release" ;;
        coverage) coverage=1; bin_dir="$probe_repo/target/llvm-cov-target/debug" ;;
        *) bin_dir="$probe_repo/target/debug" ;;
    esac
    mkdir -p "$bin_dir"
    cp "$mock_cli" "$bin_dir/edgion-cli"
    expected_cli="$bin_dir/edgion-cli"
    if [ "$case_name" = override ]; then
        profile=release
        override="$mock_cli"
        expected_cli="$mock_cli"
    fi
    record="$probe_repo/$case_name.record"
    env EDGION_TEST_PROFILE="$profile" EDGION_COVERAGE="$coverage" \
        EDGION_CLI_BIN="$override" EDGION_CLI_TEST_RECORD="$record" \
        EDGION_CONF_SYNC_CERTS_DIR="$probe_repo/certs-$case_name" \
        bash "$probe_repo/edgion-tests/integration/scripts/gen_certs/generate_conf_sync_certs.sh" \
        >/dev/null
    [ "$(wc -l < "$record" | tr -d ' ')" = 2 ]
    [ "$(sed -n '1p' "$record")" = "$expected_cli" ]
    [ "$(sed -n '2p' "$record")" = "$expected_cli" ]
done
for case_name in invalid-profile release-coverage; do
    profile=unexpected coverage=0
    if [ "$case_name" = release-coverage ]; then profile=release; coverage=1; fi
    record="$probe_repo/$case_name.record"
    if env EDGION_TEST_PROFILE="$profile" EDGION_COVERAGE="$coverage" \
        EDGION_CLI_BIN="$mock_cli" EDGION_CLI_TEST_RECORD="$record" \
        EDGION_CONF_SYNC_CERTS_DIR="$probe_repo/certs-$case_name" \
        bash "$probe_repo/edgion-tests/integration/scripts/gen_certs/generate_conf_sync_certs.sh" \
        >/dev/null 2>&1; then
        echo 'Invalid certificate artifact profile was accepted' >&2; exit 1
    fi
    [ ! -e "$record" ]
done
printf 'Certificate generator profile dispatch probes passed\n'
printf 'Artifact profile and ownership probes passed\n'
