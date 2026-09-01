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
# suite-parity: isolated auth_check
# suite-parity: isolated docker_check
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
    --k8s-dynamic-manifest "$TEST_ROOT/k8s_dynamic_scenarios.tsv"
}

write_local_runner "$TEST_ROOT/local.sh"
write_dynamic_manifest "$TEST_ROOT/test_client.rs"
write_k8s_dynamic_manifest "$TEST_ROOT/k8s_dynamic_scenarios.tsv"

write_k8s_runner "$TEST_ROOT/k8s.sh" true
pass_output="$(run_validator)"
printf '%s\n' "$pass_output" | grep -q 'PASS: K8s integration inventory covers every canonical entry.'

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

echo "validate_suite_parity_test.sh: PASS"
