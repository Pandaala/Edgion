#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RUNNER="${PROJECT_ROOT}/edgion-tests/integration/scripts/integration/run_integration.sh"
RUN_ALL="${PROJECT_ROOT}/edgion-tests/integration/scripts/integration/run_all_integration.sh"
PREPARE="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils/prepare.sh"
START="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils/start_all_with_conf.sh"
KILL="${PROJECT_ROOT}/edgion-tests/integration/scripts/utils/kill_all.sh"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

bash -n "$RUNNER" "$RUN_ALL" "$PREPARE" "$START" "$KILL"

# Exercise the selection predicate without sourcing the runner (which invokes
# main). This covers every selection form that is allowed to require Go.
selection_function=$(sed -n '/^is_external_ext_proc_required()/,/^}/p' "$RUNNER")
[ -n "$selection_function" ] || fail "selection predicate not found"
eval "$selection_function"

log_error() { :; }
log_section() { :; }
log_success() { :; }
artifact_function=$(sed -n '/^validate_external_ext_proc_artifact()/,/^}/p' "$RUNNER")
[ -n "$artifact_function" ] || fail "artifact validation function not found"
eval "$artifact_function"

test_tmp=$(mktemp -d)
test_pid=""
cleanup_test_tmp() {
    if [ -n "$test_pid" ]; then
        kill "$test_pid" 2>/dev/null || true
        wait "$test_pid" 2>/dev/null || true
    fi
    rm -rf "$test_tmp"
}
trap cleanup_test_tmp EXIT

if validate_external_ext_proc_artifact "relative-fixture"; then
    fail "relative fixture artifact was accepted"
fi
if validate_external_ext_proc_artifact "${test_tmp}/missing-fixture"; then
    fail "missing fixture artifact was accepted"
fi
: > "${test_tmp}/fixture"
chmod 755 "${test_tmp}/fixture"
validate_external_ext_proc_artifact "${test_tmp}/fixture" \
    || fail "absolute executable fixture artifact was rejected"

process_match_function=$(sed -n '/^process_matches_binary()/,/^}/p' "$KILL")
[ -n "$process_match_function" ] || fail "PID binary matching function not found"
eval "$process_match_function"
sleep_bin=$(python3 -c 'import os, shutil; print(os.path.realpath(shutil.which("sleep")))')
"$sleep_bin" 30 &
test_pid=$!
process_matches_binary "$test_pid" "$sleep_bin" \
    || fail "recorded PID did not match its exact executable"
if process_matches_binary "$test_pid" "${test_tmp}/fixture"; then
    fail "recorded PID matched an unrelated executable"
fi
kill "$test_pid"
wait "$test_pid" 2>/dev/null || true
test_pid=""

expect_required() {
    FULL_TEST=true
    G_RESOURCE="$1"
    G_ITEM="$2"
    is_external_ext_proc_required "$3" \
        || fail "fixture not selected for resource=$1 item=$2 suites=$3"
}

expect_not_required() {
    FULL_TEST="$1"
    G_RESOURCE="$2"
    G_ITEM="$3"
    if is_external_ext_proc_required "$4"; then
        fail "fixture unexpectedly selected for full=$1 resource=$2 item=$3 suites=$4"
    fi
}

expect_required "" "" ""
expect_required "EdgionPlugins" "" "base"
expect_required "EdgionPlugins" "ExtProc" "base"
expect_required "HTTPRoute" "Basic" "base,EdgionPlugins/ExtProc"
expect_not_required false "EdgionPlugins" "ExtProc" "EdgionPlugins/ExtProc"
expect_not_required true "HTTPRoute" "Basic" "base,HTTPRoute/Basic"

grep -Fq 'export EDGION_EXT_PROC_EXTERNAL_REQUIRED=1' "$RUNNER" \
    || fail "required marker is not exported by the parent runner"
grep -Fq 'EDGION_EXT_PROC_EXTERNAL_ADMIN_URL="$EXT_PROC_EXTERNAL_ADMIN_URL"' "$RUNNER" \
    || fail "admin URL is not exported by the parent runner"
grep -Fq 'EDGION_EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR="${WORK_DIR}/ext-proc-external-tls"' "$RUNNER" \
    || fail "TLS material directory is not exported only after workdir selection"
grep -Fq 'GOTOOLCHAIN=local go mod verify' "$PREPARE" \
    || fail "Go module verification is not local-toolchain-only"
grep -Fq 'GOTOOLCHAIN=local go test -mod=readonly ./...' "$PREPARE" \
    || fail "Go tests are not read-only"
grep -Fq 'must use the managed integration artifact path' "$START" \
    || fail "direct startup does not restrict the fixture to the cleanup-managed artifact"
grep -Fq 'GOTOOLCHAIN=local go build -mod=readonly -trimpath' "$PREPARE" \
    || fail "Go fixture build is not reproducible/read-only"
grep -Fq '__EXTERNAL_EXT_PROC_TARGET__' "$START" \
    || fail "external config placeholder is not rendered"
grep -Fq '__EXTERNAL_EXT_PROC_TLS_TARGET__' "$START" \
    || fail "external TLS config placeholder is not rendered"
grep -Fq '__EXTERNAL_EXT_PROC_MTLS_TARGET__' "$START" \
    || fail "external mTLS config placeholder is not rendered"
grep -Fq -- '--ext-proc-external-tls-port' "$RUNNER" \
    || fail "TLS fixture port is not passed only through the qualifying startup path"
grep -Fq -- '--ext-proc-external-mtls-port' "$RUNNER" \
    || fail "mTLS fixture port is not passed only through the qualifying startup path"
grep -Fq -- '--tls-material-dir "$EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR"' "$START" \
    || fail "fixture startup does not use the scoped workdir TLS material directory"
for secure_field in tls_capable mtls_capable tls_ready mtls_ready; do
    grep -Fq "\"${secure_field}\": True" "$RUNNER" \
        || fail "reuse readiness does not require ${secure_field}"
    grep -Fq "\"${secure_field}\": True" "$START" \
        || fail "startup readiness does not require ${secure_field}"
done
grep -Fq '"fixture_stream_capacity": 96' "$RUNNER" \
    || fail "reuse readiness does not require the fixture stream capacity"
grep -Fq '"fixture_stream_capacity": 96' "$START" \
    || fail "startup readiness does not require the fixture stream capacity"
grep -Fq 'force_kill "^${PROJECT_ROOT}/target/debug/ext_proc_external_processor([[:space:]]|$)"' "$KILL" \
    || fail "cleanup fallback is not anchored to the absolute fixture binary"

TEMPLATE="${PROJECT_ROOT}/edgion-tests/integration/fixtures/ext_proc_external/config.yaml.tmpl"
for token in \
    __EXTERNAL_EXT_PROC_TARGET__ \
    __EXTERNAL_EXT_PROC_TLS_TARGET__ \
    __EXTERNAL_EXT_PROC_MTLS_TARGET__; do
    grep -Fq "$token" "$TEMPLATE" || fail "full-only template is missing ${token}"
done
grep -Fq 'ext-proc-external-tls.example.com' "$TEMPLATE" \
    || fail "full-only TLS route is missing"
grep -Fq 'ext-proc-external-mtls.example.com' "$TEMPLATE" \
    || fail "full-only mTLS route is missing"
if grep -R -Fq 'ext-proc-external-tls.example.com' \
    "${PROJECT_ROOT}/edgion-tests/integration/conf"; then
    fail "external TLS fixture leaked into fast-suite YAML"
fi
if grep -R -Fq 'ext-proc-external-mtls.example.com' \
    "${PROJECT_ROOT}/edgion-tests/integration/conf"; then
    fail "external mTLS fixture leaked into fast-suite YAML"
fi

# Execute the render function against a private workdir and a no-op CLI. This
# proves all three target classes are replaced without exposing Secret bodies.
render_function=$(sed -n '/^render_and_load_external_ext_proc_config()/,/^}/p' "$START")
[ -n "$render_function" ] || fail "external config render function not found"
eval "$render_function"
CONFIG_DIR="${test_tmp}/config"
EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR="${test_tmp}/materials"
EXT_PROC_EXTERNAL_CONFIG_TEMPLATE="$TEMPLATE"
EXT_PROC_EXTERNAL_BIN="${test_tmp}/fixture"
EXT_PROC_EXTERNAL_GRPC_PORT=30044
EXT_PROC_EXTERNAL_TLS_PORT=30046
EXT_PROC_EXTERNAL_MTLS_PORT=30047
CONTROLLER_ADMIN_PORT=12101
BIN_DIR="${test_tmp}/bin"
mkdir -p "$CONFIG_DIR" "$EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR" "$BIN_DIR"
: > "${EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR}/ext-proc-external-server-ca.yaml"
: > "${EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR}/ext-proc-external-client-initial.yaml"
cat > "${BIN_DIR}/edgion-cli" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod 755 "${BIN_DIR}/edgion-cli"
render_and_load_external_ext_proc_config \
    || fail "external TLS config rendering contract failed"
rendered="${CONFIG_DIR}/ext_proc_external.yaml"
grep -Fq 'target: "127.0.0.1:30044"' "$rendered" \
    || fail "plain fixture target was not rendered"
grep -Fq 'target: "127.0.0.1:30046"' "$rendered" \
    || fail "TLS fixture target was not rendered"
grep -Fq 'target: "127.0.0.1:30047"' "$rendered" \
    || fail "mTLS fixture target was not rendered"
if grep -Eq '__EXTERNAL_EXT_PROC_[A-Z_]*TARGET__' "$rendered"; then
    fail "rendered external config retained a target placeholder"
fi

ordinary_line=$(grep -n '^[[:space:]]*load_configs$' "$START" | tail -1 | cut -d: -f1)
external_line=$(grep -n '^[[:space:]]*render_and_load_external_ext_proc_config$' "$START" | tail -1 | cut -d: -f1)
secrets_line=$(grep -n '^[[:space:]]*load_generated_secrets$' "$START" | tail -1 | cut -d: -f1)
gateway_line=$(grep -n '^[[:space:]]*start_gateway$' "$START" | tail -1 | cut -d: -f1)
if [ "$ordinary_line" -ge "$external_line" ] \
    || [ "$external_line" -ge "$secrets_line" ] \
    || [ "$secrets_line" -ge "$gateway_line" ]; then
    fail "config/startup order is not ordinary -> external -> secrets -> gateway"
fi

echo "OK: external ExtProc runner wiring contracts passed"
