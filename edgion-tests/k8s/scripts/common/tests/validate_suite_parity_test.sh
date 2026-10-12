#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VALIDATOR="${SCRIPT_DIR}/../validate_suite_parity.sh"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

write_local_runner() {
  local path="$1"
  cat > "$path" <<'EOF'
run_dynamic_scenarios() {
    ids=$(test_client --list-dynamic-scenarios)
}
run_isolated_scenarios() {
    local names=(
        "auth_check"
    )
    names+=("docker_check")
}
# =============================================================================
# Cleanup function
cleanup() {
    :
}
run_all_tests() {
    run_test "Alpha_One" "test_client -r Alpha -i One"
    run_test "SpecialAlias_Item" "test_client -r SpecialAlias -i Item"
    run_test "Ignored_${selected}" "test_client"
}
main() {
    report_test "Final_MetricsLabelWhitelist" "PASS"
    report_test "Final_UnknownConfigFields" "PASS"
    report_test "Final_RequeueDeadLoop" "PASS"
    report_test "Final_CacheResidue" "PASS"
}
EOF
}

write_k8s_runner() {
  local path="$1"
  local include_all="$2"
  cat > "$path" <<'EOF'
suite_test_name() {
  local resource="$1"
  local item="$2"
  local safe_item
  safe_item="$(echo "${item}" | tr '/' '_')"
  echo "${resource}_${safe_item}"
}
run_all_selected_suites() {
  local suites=(
    "Alpha/One|Alpha|One"
EOF
  if [ "$include_all" = "true" ]; then
    cat >> "$path" <<'EOF'
    "SpecialAlias/Item|SpecialAlias|Item"
EOF
  fi
  cat >> "$path" <<'EOF'
  )
  local local_start_from=""
}
run_dynamic() {
  test_client --dynamic-scenario DynOne
}
RUN_DYNAMIC_SCRIPT="common/run_k8s_dynamic_scenarios.sh"
K8S_DYNAMIC_MANIFEST="common/k8s_dynamic_scenarios.tsv"
run_manifest_dynamic_scenarios() {
  "$RUN_DYNAMIC_SCRIPT"
}
if ! run_manifest_dynamic_scenarios; then
  exit 1
fi
run_all_isolated_scenarios() {
  # suite-parity: isolated auth_check
  # suite-parity: isolated docker_check
  printf '%s\n' 'Isolated|auth_check' 'Isolated|docker_check' > jobs.expected
  run_isolated_helper_group jobs jobs.expected helper \
    --job auth_check \
    --job docker_check
}
# suite-parity: final-gate MetricsLabelWhitelist
# suite-parity: final-gate UnknownConfigFields
# suite-parity: final-gate RequeueDeadLoop
# suite-parity: final-gate CacheResidue
EOF
}

write_dynamic_manifest() {
  local path="$1"
  cat > "$path" <<'EOF'
const EXPECTED_DYNAMIC_SCENARIO_IDS: &[&str] = &[
    "DynOne",
    "DynManifest",
];
EOF
}

write_k8s_dynamic_manifest() {
  local path="$1"
  cat > "$path" <<'EOF'
# scenario_id	k8s_conf_path	reset_path	kubectl_resource	gateway_kind	namespace	name
DynManifest	Feature/DynamicTest	initial	examples.example.io	examples	default	dynamic-example
EOF
}

run_validator() {
  "$VALIDATOR" \
    --local-runner "$TEST_ROOT/local.sh" \
    --k8s-runner "$TEST_ROOT/k8s.sh" \
    --dynamic-manifest-source "$TEST_ROOT/test_client.rs" \
    --k8s-dynamic-manifest "$TEST_ROOT/k8s_dynamic_scenarios.tsv" \
    --scope-exceptions "$TEST_ROOT/scope_exceptions.tsv"
}

write_local_runner "$TEST_ROOT/local.sh"
write_dynamic_manifest "$TEST_ROOT/test_client.rs"
write_k8s_dynamic_manifest "$TEST_ROOT/k8s_dynamic_scenarios.tsv"
: > "$TEST_ROOT/scope_exceptions.tsv"

write_k8s_runner "$TEST_ROOT/k8s.sh" true
pass_output="$(run_validator)"
printf '%s\n' "$pass_output" | grep -q 'PASS: K8s integration inventory covers every canonical entry.'

sed -i.bak '/--job docker_check/d' "$TEST_ROOT/k8s.sh"
rm "$TEST_ROOT/k8s.sh.bak"
set +e
missing_job_output="$(run_validator 2>&1)"
missing_job_rc=$?
set -e
[ "$missing_job_rc" -eq 2 ]
printf '%s\n' "$missing_job_output" | grep -q 'isolated invocations and expected results do not match'

write_k8s_runner "$TEST_ROOT/k8s.sh" true
sed -i.bak 's/--job docker_check/--job auth_check/' "$TEST_ROOT/k8s.sh"
rm "$TEST_ROOT/k8s.sh.bak"
set +e
duplicate_job_output="$(run_validator 2>&1)"
duplicate_job_rc=$?
set -e
[ "$duplicate_job_rc" -eq 2 ]
printf '%s\n' "$duplicate_job_output" | grep -q 'isolated invocations: duplicate entries'

write_k8s_runner "$TEST_ROOT/k8s.sh" true
sed -i.bak "s/ 'Isolated|docker_check'//" "$TEST_ROOT/k8s.sh"
rm "$TEST_ROOT/k8s.sh.bak"
set +e
missing_expected_output="$(run_validator 2>&1)"
missing_expected_rc=$?
set -e
[ "$missing_expected_rc" -eq 2 ]
printf '%s\n' "$missing_expected_output" | grep -q 'isolated invocations and expected results do not match'

write_k8s_runner "$TEST_ROOT/k8s.sh" true
sed -i.bak '/--job /d' "$TEST_ROOT/k8s.sh"
rm "$TEST_ROOT/k8s.sh.bak"
set +e
marker_only_output="$(run_validator 2>&1)"
marker_only_rc=$?
set -e
[ "$marker_only_rc" -eq 2 ]
printf '%s\n' "$marker_only_output" | grep -q 'isolated invocations and expected results do not match'

write_k8s_runner "$TEST_ROOT/k8s.sh" true
sed -i.bak '/suite-parity: final-gate/d' "$TEST_ROOT/k8s.sh"
rm "$TEST_ROOT/k8s.sh.bak"
set +e
missing_output="$(run_validator 2>&1)"
missing_rc=$?
set -e
[ "$missing_rc" -eq 1 ]
printf '%s\n' "$missing_output" | grep -q 'MISSING final-gate MetricsLabelWhitelist'
printf '%s\n' "$missing_output" | grep -q 'FAIL: K8s integration inventory is missing canonical coverage.'

write_k8s_runner "$TEST_ROOT/k8s.sh" true
: > "$TEST_ROOT/k8s_dynamic_scenarios.tsv"
set +e
manifest_output="$(run_validator 2>&1)"
manifest_rc=$?
set -e
[ "$manifest_rc" -eq 2 ]
printf '%s\n' "$manifest_output" | grep -q 'K8s dynamic manifest contains no scenarios'

write_k8s_runner "$TEST_ROOT/k8s.sh" true
sed -i.bak '/if ! run_manifest_dynamic_scenarios; then/,+2d' "$TEST_ROOT/k8s.sh"
rm "$TEST_ROOT/k8s.sh.bak"
write_k8s_dynamic_manifest "$TEST_ROOT/k8s_dynamic_scenarios.tsv"
set +e
invocation_output="$(run_validator 2>&1)"
invocation_rc=$?
set -e
[ "$invocation_rc" -eq 2 ]
printf '%s\n' "$invocation_output" | grep -q 'does not invoke the manifest-backed dynamic phase fail-closed'

write_k8s_runner "$TEST_ROOT/k8s.sh" false
write_k8s_dynamic_manifest "$TEST_ROOT/k8s_dynamic_scenarios.tsv"
sed -i.bak 's/"Alpha\/One|Alpha|One"/not-a-literal-suite/' "$TEST_ROOT/k8s.sh"
rm "$TEST_ROOT/k8s.sh.bak"
set +e
malformed_output="$(run_validator 2>&1)"
malformed_rc=$?
set -e
[ "$malformed_rc" -eq 2 ]
printf '%s\n' "$malformed_output" | grep -q 'malformed or non-literal entries'

write_k8s_runner "$TEST_ROOT/k8s.sh" false
mkdir -p "$TEST_ROOT/repo/edgion-tests/k8s/scripts/common" \
  "$TEST_ROOT/repo/edgion-tests/src/client/suites" \
  "$TEST_ROOT/repo/edgion-tests/integration/scripts/integration/ldap_egress"
cp "$VALIDATOR" "$TEST_ROOT/repo/edgion-tests/k8s/scripts/common/validate_suite_parity.sh"
cat > "$TEST_ROOT/repo/edgion-tests/src/client/suites/l4_duplex.rs" <<'EOF'
struct LogProbe;
fn access_log_path_is_host_owned() { let host = "host"; }
EOF
cat > "$TEST_ROOT/repo/edgion-tests/integration/scripts/integration/ldap_egress/scenario.py" <<'EOF'
class Scenario:
    """OpenLDAP fixture executed through docker."""
EOF
cat > "$TEST_ROOT/repo/out-of-scope.rs" <<'EOF'
struct LogProbe;
fn access_log_path_is_host_owned() { let host = "host"; }
EOF

VALID_RATIONALE="The canonical assertion requires a gateway-owned host file that cannot be mounted into the Kubernetes client execution environment."
VALID_TRIGGER="Re-evaluate when the Kubernetes harness exposes a canonical authenticated gateway log evidence channel to the test client pod."
VALID_PATH="edgion-tests/src/client/suites/l4_duplex.rs"

run_repo_validator() {
  "$TEST_ROOT/repo/edgion-tests/k8s/scripts/common/validate_suite_parity.sh" \
    --local-runner "$TEST_ROOT/local.sh" --k8s-runner "$TEST_ROOT/k8s.sh" \
    --dynamic-manifest-source "$TEST_ROOT/test_client.rs" \
    --k8s-dynamic-manifest "$TEST_ROOT/k8s_dynamic_scenarios.tsv" \
    --scope-exceptions "$TEST_ROOT/scope_exceptions.tsv"
}

write_exception() {
  printf '%s\n' "$1" > "$TEST_ROOT/scope_exceptions.tsv"
}

expect_exception_error() {
  local row="$1" expected="$2" output rc
  write_exception "$row"
  set +e
  output="$(run_repo_validator 2>&1)"
  rc=$?
  set -e
  [ "$rc" -eq 2 ] || { echo "expected exception validation error, rc=$rc: $output" >&2; exit 1; }
  printf '%s\n' "$output" | grep -Fq "$expected" \
    || { echo "expected exception error containing '$expected': $output" >&2; exit 1; }
}

valid_row="static\tSpecialAlias/Item\thost_log_topology\t${VALID_PATH}\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}"
write_exception "$(printf '%b' "$valid_row")"
exception_output="$(run_repo_validator)"
printf '%s\n' "$exception_output" | grep -q 'EXCEPTED static SpecialAlias/Item'

expect_exception_error "$(printf '%b' "static\tSpecialAlias/*\thost_log_topology\t${VALID_PATH}\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "invalid literal id"
expect_exception_error "$(printf '%b\n%b' "$valid_row" "$valid_row")" "duplicate exception"
expect_exception_error "$(printf '%b' "static\tMissing/Thing\thost_log_topology\t${VALID_PATH}\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "noncanonical or stale id"
expect_exception_error "$(printf '%b' "static\tAlpha/One\thost_log_topology\t${VALID_PATH}\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "already has K8s coverage"
expect_exception_error "$(printf '%b' "unknown\tSpecialAlias/Item\thost_log_topology\t${VALID_PATH}\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "invalid category"
expect_exception_error "$(printf '%b' "final-gate\tMetricsLabelWhitelist\thost_log_topology\t${VALID_PATH}\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "invalid category"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\tunknown_reason\t${VALID_PATH}\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "unknown reason"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\t/tmp/evidence.rs\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "invalid evidence path"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\t../evidence.rs\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "invalid evidence path"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\tmissing/evidence.rs\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "evidence file is missing"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\tout-of-scope.rs\tLogProbe\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "outside the approved scope"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\t${VALID_PATH}\tWrongSymbol\t${VALID_RATIONALE}\t${VALID_TRIGGER}")" "evidence symbol is outside"
cp "$TEST_ROOT/repo/$VALID_PATH" "$TEST_ROOT/repo/$VALID_PATH.save"
printf 'struct LogProbe;\n' > "$TEST_ROOT/repo/$VALID_PATH"
expect_exception_error "$(printf '%b' "$valid_row")" "evidence no longer supports"
mv "$TEST_ROOT/repo/$VALID_PATH.save" "$TEST_ROOT/repo/$VALID_PATH"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\t${VALID_PATH}\tLogProbe\tshort reason\t${VALID_TRIGGER}")" "rationale is blank"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\t${VALID_PATH}\tLogProbe\tThis rationale remains TBD until somebody investigates the exact topology carefully.\t${VALID_TRIGGER}")" "rationale is blank"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\t${VALID_PATH}\tLogProbe\tEvery Kubernetes test and scenario lacks this capability across the entire repository today.\t${VALID_TRIGGER}")" "rationale is blank"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\t${VALID_PATH}\tLogProbe\t${VALID_RATIONALE}\twhen logs exist")" "re-evaluation trigger is invalid"
expect_exception_error "$(printf '%b' "static\tSpecialAlias/Item\thost_log_topology\t${VALID_PATH}\tLogProbe\t${VALID_RATIONALE}\tRe-evaluate when this TODO is handled by another future change in the harness.")" "re-evaluation trigger is invalid"

echo "validate_suite_parity_test.sh: PASS"
