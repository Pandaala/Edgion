#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_RUNNER="${SCRIPT_DIR}/../../run_k8s_integration.sh"
BATCH_VALIDATOR="${SCRIPT_DIR}/../validate_batch_results.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-k8s-dynamic-integration-test.XXXXXX)"
FUNCTION_FILE="${TEMP_DIR}/dynamic-function.sh"
STARTUP_FUNCTION_FILE="${TEMP_DIR}/startup-function.sh"
FAKE_RUNNER="${TEMP_DIR}/fake-dynamic-runner.sh"
FAKE_FINAL_RUNNER="${TEMP_DIR}/fake-final-runner.sh"
FINAL_GATE_LOG="${TEMP_DIR}/final-gates.log"
STARTUP_LOG="${TEMP_DIR}/startup.log"
SERVER_IDS="${TEMP_DIR}/server-ids"
FAKE_BIN="${TEMP_DIR}/bin"
K8S_DYNAMIC_MANIFEST="${TEMP_DIR}/k8s_dynamic_scenarios.tsv"
CONF_ROOT="${TEMP_DIR}/conf"

cleanup() {
  rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT INT TERM

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

awk '
  /^run_manifest_dynamic_scenarios\(\) \{/ { emit = 1 }
  /^run_all_selected_suites\(\) \{/ { emit = 0 }
  emit { print }
' "${MAIN_RUNNER}" > "${FUNCTION_FILE}"

if ! grep -Fq 'run_manifest_dynamic_scenarios()' "${FUNCTION_FILE}"; then
  fail "could not extract run_manifest_dynamic_scenarios from the main runner"
fi

awk '
  /^restart_gateway_after_initial_fixtures\(\) \{/ { emit = 1 }
  /^trigger_reload\(\) \{/ { emit = 0 }
  emit { print }
' "${MAIN_RUNNER}" > "${STARTUP_FUNCTION_FILE}"
if ! grep -Fq 'restart_gateway_after_initial_fixtures()' "${STARTUP_FUNCTION_FILE}"; then
  fail "could not extract post-fixture Gateway restart function"
fi

# shellcheck source=../validate_batch_results.sh
source "${BATCH_VALIDATOR}"
# shellcheck source=/dev/null
source "${FUNCTION_FILE}"
# shellcheck source=/dev/null
source "${STARTUP_FUNCTION_FILE}"

mkdir -p "${CONF_ROOT}"
cat > "${K8S_DYNAMIC_MANIFEST}" <<'EOF'
BasicAuthDynamic	EdgionPlugins/BasicAuth/DynamicTest	initial	edgionplugins.edgion.io	edgionplugins	edgion-default	basic-auth-dynamic
BasicAuthDynamicGrouped	EdgionPlugins/BasicAuth/DynamicTest	initial_grouped	edgionplugins.edgion.io	edgionplugins	edgion-default	basic-auth-dynamic-grouped
RateLimitLocalDynamic	EdgionPlugins/RateLimitLocal/DynamicTest	initial	edgionplugins.edgion.io	edgionplugins	edgion-default	rate-limit-dynamic
EOF

cat > "${FAKE_RUNNER}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${FAKE_DYNAMIC_MODE:-success}" in
  success)
    echo '@@DYNAMIC_RESULT BasicAuthDynamic PASS 0'
    echo '@@DYNAMIC_RESULT BasicAuthDynamicGrouped PASS 0'
    echo '@@DYNAMIC_RESULT RateLimitLocalDynamic PASS 0'
    echo '@@DYNAMIC_SUMMARY total=3 failed=0'
    ;;
  scenario-failure)
    echo '@@DYNAMIC_RESULT BasicAuthDynamic PASS 0'
    echo '@@DYNAMIC_RESULT BasicAuthDynamicGrouped PASS 0'
    echo '@@DYNAMIC_RESULT RateLimitLocalDynamic FAIL 7'
    echo '@@DYNAMIC_SUMMARY total=3 failed=1'
    exit 1
    ;;
  malformed)
    echo '@@DYNAMIC_RESULT BasicAuthDynamic PASS 0'
    ;;
  *)
    exit 2
    ;;
esac
EOF
chmod +x "${FAKE_RUNNER}"
RUN_DYNAMIC_SCRIPT="${FAKE_RUNNER}"

cat > "${FAKE_FINAL_RUNNER}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

gate_id="${0##*/}"
gate_id="${gate_id#fake-}"
printf '%s %s\n' "${gate_id}" "$*" >> "${FINAL_GATE_LOG}"
case "${gate_id}" in
  MetricsLabelWhitelist) metadata='endpoints=2 checked=2' ;;
  UnknownConfigFields) metadata='total=0 items=0 truncated=false' ;;
  RequeueDeadLoop) metadata='pods=1 streams=1 matches=0' ;;
  CacheResidue) metadata='validator_rc=0 reason=clean' ;;
  *) exit 2 ;;
esac
case "${FAKE_FINAL_MODE:-success}:${gate_id}" in
  failure:UnknownConfigFields)
    echo "@@FINAL_GATE_RESULT ${gate_id} FAIL total=1 items=1 truncated=false"
    echo '@@FINAL_GATE_SUMMARY total=1 failed=1'
    exit 1
    ;;
  malformed:MetricsLabelWhitelist)
    echo "@@FINAL_GATE_RESULT ${gate_id} PASS ${metadata}"
    echo '@@FINAL_GATE_SUMMARY total=2 failed=0'
    ;;
  unknown-marker:MetricsLabelWhitelist)
    echo "@@FINAL_GATE_RESULT ${gate_id} PASS ${metadata}"
    echo '@@FINAL_GATE_DIAGNOSTIC unexpected=true'
    echo '@@FINAL_GATE_SUMMARY total=1 failed=0'
    ;;
  *)
    echo "@@FINAL_GATE_RESULT ${gate_id} PASS ${metadata}"
    echo '@@FINAL_GATE_SUMMARY total=1 failed=0'
    ;;
esac
EOF
chmod +x "${FAKE_FINAL_RUNNER}"
for gate_id in MetricsLabelWhitelist UnknownConfigFields RequeueDeadLoop CacheResidue; do
  cp "${FAKE_FINAL_RUNNER}" "${TEMP_DIR}/fake-${gate_id}"
done
RUN_METRICS_LABEL_GATE="${TEMP_DIR}/fake-MetricsLabelWhitelist"
RUN_UNKNOWN_CONFIG_FIELDS_GATE="${TEMP_DIR}/fake-UnknownConfigFields"
RUN_REQUEUE_DEAD_LOOP_GATE="${TEMP_DIR}/fake-RequeueDeadLoop"
RUN_CACHE_RESIDUE_GATE="${TEMP_DIR}/fake-CacheResidue"
RUN_START_TIME="2026-08-30T12:34:56Z"
export FINAL_GATE_LOG

SELECTED_COUNT=0
EXECUTED_COUNT=0
FAIL_COUNT=0
FAILED_SUITES=()
FAKE_DYNAMIC_MODE=success run_manifest_dynamic_scenarios >/dev/null \
  || fail "valid dynamic protocol was rejected"
[[ "${SELECTED_COUNT}" -eq 3 ]] || fail "success selected count mismatch"
[[ "${EXECUTED_COUNT}" -eq 3 ]] || fail "success executed count mismatch"
[[ "${FAIL_COUNT}" -eq 0 ]] || fail "success failure count mismatch"

SELECTED_COUNT=0
EXECUTED_COUNT=0
FAIL_COUNT=0
FAILED_SUITES=()
FAKE_DYNAMIC_MODE=scenario-failure run_manifest_dynamic_scenarios >/dev/null \
  || fail "valid protocol containing a scenario failure was rejected as a harness error"
[[ "${SELECTED_COUNT}" -eq 3 ]] || fail "failure selected count mismatch"
[[ "${EXECUTED_COUNT}" -eq 3 ]] || fail "failure executed count mismatch"
[[ "${FAIL_COUNT}" -eq 1 ]] || fail "failure count mismatch"
[[ "${FAILED_SUITES[0]}" == "Dynamic/RateLimitLocalDynamic" ]] \
  || fail "failed dynamic scenario was not recorded"

SELECTED_COUNT=0
EXECUTED_COUNT=0
FAIL_COUNT=0
FAILED_SUITES=()
if FAKE_DYNAMIC_MODE=malformed run_manifest_dynamic_scenarios >/dev/null 2>&1; then
  fail "malformed dynamic result protocol was accepted"
fi
[[ "${SELECTED_COUNT}" -eq 3 ]] || fail "malformed protocol selected count mismatch"
[[ "${EXECUTED_COUNT}" -eq 0 ]] || fail "malformed protocol changed executed count"

SELECTED_COUNT=0
EXECUTED_COUNT=0
FAIL_COUNT=0
FAILED_SUITES=()
: > "${FINAL_GATE_LOG}"
FAKE_FINAL_MODE=success run_all_final_gates >/dev/null \
  || fail "valid final-gate protocols were rejected"
[[ "${SELECTED_COUNT}" -eq 4 ]] || fail "final-gate selected count mismatch"
[[ "${EXECUTED_COUNT}" -eq 4 ]] || fail "final-gate executed count mismatch"
[[ "${FAIL_COUNT}" -eq 0 ]] || fail "final-gate failure count mismatch"
expected_order="MetricsLabelWhitelist
UnknownConfigFields
RequeueDeadLoop
CacheResidue"
actual_order="$(awk '{print $1}' "${FINAL_GATE_LOG}")"
[[ "${actual_order}" == "${expected_order}" ]] || fail "final gates did not run in canonical order"
grep -Fq 'RequeueDeadLoop --run-start-time 2026-08-30T12:34:56Z' "${FINAL_GATE_LOG}" \
  || fail "RequeueDeadLoop did not receive the captured run-start timestamp"
[[ "$(tail -n 1 "${FINAL_GATE_LOG}" | awk '{print $1}')" == "CacheResidue" ]] \
  || fail "CacheResidue was not the final gate"

SELECTED_COUNT=0
EXECUTED_COUNT=0
FAIL_COUNT=0
FAILED_SUITES=()
FAKE_FINAL_MODE=failure run_final_gate UnknownConfigFields "${RUN_UNKNOWN_CONFIG_FIELDS_GATE}" >/dev/null \
  || fail "valid final-gate FAIL protocol was treated as a harness error"
[[ "${SELECTED_COUNT}" -eq 1 && "${EXECUTED_COUNT}" -eq 1 && "${FAIL_COUNT}" -eq 1 ]] \
  || fail "failed final gate was not strictly accounted"
[[ "${FAILED_SUITES[0]}" == "Final/UnknownConfigFields" ]] \
  || fail "failed final gate was not recorded"

SELECTED_COUNT=0
EXECUTED_COUNT=0
FAIL_COUNT=0
FAILED_SUITES=()
if FAKE_FINAL_MODE=malformed run_final_gate MetricsLabelWhitelist "${RUN_METRICS_LABEL_GATE}" \
  >/dev/null 2>&1; then
  fail "malformed final-gate protocol was accepted"
fi
[[ "${SELECTED_COUNT}" -eq 1 && "${EXECUTED_COUNT}" -eq 0 ]] \
  || fail "malformed final-gate protocol changed execution accounting"

SELECTED_COUNT=0
EXECUTED_COUNT=0
FAIL_COUNT=0
FAILED_SUITES=()
if FAKE_FINAL_MODE=unknown-marker run_final_gate MetricsLabelWhitelist "${RUN_METRICS_LABEL_GATE}" \
  >/dev/null 2>&1; then
  fail "unknown final-gate protocol marker was accepted"
fi
[[ "${SELECTED_COUNT}" -eq 1 && "${EXECUTED_COUNT}" -eq 0 ]] \
  || fail "unknown final-gate marker changed execution accounting"

for triple in \
  'EdgionTls/port_only|EdgionTls|port_only' \
  'EdgionTls/sni_collision|EdgionTls|sni_collision' \
  'EdgionTls/wildcard_single_label|EdgionTls|wildcard_single_label' \
  'HTTPRoute/Backend/DnsResolution|HTTPRoute|Backend/DnsResolution' \
  'HTTPRoute/Backend/LBEwma|HTTPRoute|Backend/LBEwma' \
  'HTTPRoute/Backend/LBEwmaRetryOnStatus|HTTPRoute|Backend/LBEwmaRetryOnStatus' \
  'HTTPRoute/Backend/DegradeThreshold|HTTPRoute|Backend/DegradeThreshold' \
  'HTTPRoute/DanglingExtensionRef|HTTPRoute|DanglingExtensionRef' \
  'HTTPRoute/Delegation|HTTPRoute|Delegation' \
  'HTTPRoute/Filters/PercentMirror|HTTPRoute|Filters/PercentMirror' \
  'HTTPRoute/SseStreaming|HTTPRoute|SseStreaming' \
  'HTTPRoute/Transport|HTTPRoute|Transport' \
  'Gateway/TLS/MisdirectedRequest|Gateway|TLS/MisdirectedRequest' \
  'Status/EdgionBackendTrafficPolicy|Status|EdgionBackendTrafficPolicy' \
  'Status/GRPCRoute|Status|GRPCRoute' \
  'Status/HostnameIntersection|Status|HostnameIntersection' \
  'Status/RefGrant/EdgionConfigData|Status|RefGrant/EdgionConfigData'; do
  grep -Fq "\"${triple}\"" "${MAIN_RUNNER}" \
    || fail "main runner is missing static suite triple: ${triple}"
done

mkdir -p "${FAKE_BIN}"
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'KUBECTL:%s\n' "$*" >> "${STARTUP_LOG}"
EOF
chmod +x "${FAKE_BIN}/kubectl"

get_gateway_server_info() {
  printf 'SERVER_INFO:controller-epoch:true\n' >> "${STARTUP_LOG}"
  printf 'controller-epoch\ttrue\n'
}
get_gateway_pod_uid() {
  local server_id
  server_id="$(head -n 1 "${SERVER_IDS}")"
  sed '1d' "${SERVER_IDS}" > "${SERVER_IDS}.next"
  mv "${SERVER_IDS}.next" "${SERVER_IDS}"
  printf 'POD_UID:%s\n' "${server_id}" >> "${STARTUP_LOG}"
  printf '%s\n' "${server_id}"
}
install_local_core_binaries() {
  echo 'INSTALL_LOCAL_CORE' >> "${STARTUP_LOG}"
}
wait_gateway_stable() {
  printf 'WAIT_STABLE:%s\n' "$1" >> "${STARTUP_LOG}"
}
export STARTUP_LOG SERVER_IDS

: > "${STARTUP_LOG}"
printf 'gateway-before\ngateway-after\n' > "${SERVER_IDS}"
PATH="${FAKE_BIN}:${PATH}" restart_gateway_after_initial_fixtures >/dev/null \
  || fail "post-fixture Gateway restart rejected changed identities"
expected_startup_order="POD_UID:gateway-before
KUBECTL:rollout restart deployment/edgion-gateway -n edgion-system
KUBECTL:rollout status deployment/edgion-gateway -n edgion-system --timeout=300s
INSTALL_LOCAL_CORE
WAIT_STABLE:300
POD_UID:gateway-after
SERVER_INFO:controller-epoch:true"
[[ "$(cat "${STARTUP_LOG}")" == "${expected_startup_order}" ]] \
  || fail "post-fixture Gateway restart order or bounded convergence changed"

printf 'same-id\nsame-id\n' > "${SERVER_IDS}"
if PATH="${FAKE_BIN}:${PATH}" restart_gateway_after_initial_fixtures >/dev/null 2>&1; then
  fail "post-fixture Gateway restart accepted an unchanged Pod UID"
fi

printf 'gateway-before\ngateway-after\n' > "${SERVER_IDS}"
get_gateway_server_info() {
  printf 'controller-epoch\tfalse\n'
}
if PATH="${FAKE_BIN}:${PATH}" restart_gateway_after_initial_fixtures >/dev/null 2>&1; then
  fail "post-fixture Gateway restart accepted a not-ready sync state"
fi

apply_line="$(grep -nF '"${APPLY_ALL_SCRIPT}" "${CONF_ROOT}"' "${MAIN_RUNNER}" | tail -n 1 | cut -d: -f1)"
restart_line="$(grep -nF 'restart_gateway_after_initial_fixtures' "${MAIN_RUNNER}" | tail -n 1 | cut -d: -f1)"
phase_two_line="$(grep -nF 'echo "Phase 2/2: Run tests"' "${MAIN_RUNNER}" | cut -d: -f1)"
[[ "${apply_line}" -lt "${restart_line}" && "${restart_line}" -lt "${phase_two_line}" ]] \
  || fail "post-fixture Gateway restart is not between initial apply and tests"

echo "OK: K8s main-runner dynamic/final integration tests passed"
