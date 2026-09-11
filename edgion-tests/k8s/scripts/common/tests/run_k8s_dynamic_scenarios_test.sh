#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNNER="${SCRIPT_DIR}/../run_k8s_dynamic_scenarios.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-k8s-dynamic-test.XXXXXX)"
FAKE_BIN="${TEMP_DIR}/bin"
KUBECTL_LOG="${TEMP_DIR}/kubectl.log"
SAFE_CONF="${TEMP_DIR}/safe-conf"
FORBIDDEN_CONF="${TEMP_DIR}/forbidden-conf"
PORTS_CONFIG="${TEMP_DIR}/ports.json"
BASE_DYNAMIC_MANIFEST="${TEMP_DIR}/base-dynamic-scenarios.tsv"
CLUSTER_DYNAMIC_MANIFEST="${TEMP_DIR}/cluster-dynamic-scenarios.tsv"
ENDPOINT_DYNAMIC_MANIFEST="${TEMP_DIR}/endpoint-dynamic-scenarios.tsv"
FAKE_ENDPOINT_INVENTORY="${TEMP_DIR}/fake-endpoint-inventory.sh"
SYSTEM_TAR="$(command -v tar)"

cleanup() {
  rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT INT TERM

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local expected="$2"
  if ! grep -Fq -- "${expected}" "${file}"; then
    echo "ERROR: expected output to contain: ${expected}" >&2
    cat "${file}" >&2
    exit 1
  fi
}

assert_not_contains() {
  local file="$1"
  local unexpected="$2"
  if grep -Fq -- "${unexpected}" "${file}"; then
    echo "ERROR: expected output not to contain: ${unexpected}" >&2
    cat "${file}" >&2
    exit 1
  fi
}

write_fixture() {
  local path="$1"
  local name="$2"
  mkdir -p "$(dirname "${path}")"
  cat > "${path}" <<EOF
apiVersion: v1
kind: ConfigMap
metadata:
  name: ${name}
  namespace: test
EOF
}

write_fixture "${SAFE_CONF}/EdgionPlugins/BasicAuth/DynamicTest/initial/resource.yaml" "basic-auth-k8s"
write_fixture "${SAFE_CONF}/EdgionPlugins/BasicAuth/DynamicTest/initial_grouped/resource.yaml" "basic-auth-grouped-k8s"
write_fixture "${SAFE_CONF}/EdgionPlugins/RateLimitLocal/DynamicTest/initial/resource.yaml" "rate-limit-k8s"
printf '{"current_max":1,"suites":{}}\n' > "${PORTS_CONFIG}"
cat > "${BASE_DYNAMIC_MANIFEST}" <<'EOF'
# scenario_id	k8s_conf_path	reset_path	kubectl_resource	gateway_kind	namespace	name
BasicAuthDynamic	EdgionPlugins/BasicAuth/DynamicTest	initial	edgionplugins.edgion.io	edgionplugins	edgion-default	basic-auth-dynamic
BasicAuthDynamicGrouped	EdgionPlugins/BasicAuth/DynamicTest	initial_grouped	edgionplugins.edgion.io	edgionplugins	edgion-default	basic-auth-dynamic-grouped
RateLimitLocalDynamic	EdgionPlugins/RateLimitLocal/DynamicTest	initial	edgionplugins.edgion.io	edgionplugins	edgion-default	rate-limit-dynamic
EOF
export DYNAMIC_MANIFEST="${BASE_DYNAMIC_MANIFEST}"

cat > "${CLUSTER_DYNAMIC_MANIFEST}" <<'EOF'
# scenario_id	k8s_conf_path	reset_path	kubectl_resource	gateway_kind	namespace	name
GcParamsFlap	Status/GatewayClassParamsRef/RefFlap/DynamicTest	initial	gatewayclasses.gateway.networking.k8s.io	gatewayclass	-	params-flap-class
EOF
write_fixture \
  "${SAFE_CONF}/Status/GatewayClassParamsRef/RefFlap/DynamicTest/initial/resource.yaml" \
  "params-flap-class"
cat > "${SAFE_CONF}/Status/GatewayClassParamsRef/RefFlap/DynamicTest/initial/reset-delete.txt" <<'EOF'
edgiongatewayclassconfigs.edgion.io//params-flap-egc
EOF

cat > "${ENDPOINT_DYNAMIC_MANIFEST}" <<'EOF'
# scenario_id	k8s_conf_path	reset_path	kubectl_resource	gateway_kind	namespace	name
SpWeighted	HTTPRoute/Backend/SessionPersistenceWeighted/DynamicTest	initial	httproutes.gateway.networking.k8s.io	httproute	edgion-default	spw-route
EOF
write_fixture \
  "${SAFE_CONF}/HTTPRoute/Backend/SessionPersistenceWeighted/DynamicTest/initial/resource.yaml" \
  "spw-route"
cat > "${SAFE_CONF}/HTTPRoute/Backend/SessionPersistenceWeighted/DynamicTest/initial/test-server.yaml" <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: dynamic-test-server
  namespace: test
  labels:
    edgion.io/test-server-image: "true"
spec:
  template:
    spec:
      containers:
        - name: test-server
          image: stale:test
          imagePullPolicy: IfNotPresent
EOF
cat > "${SAFE_CONF}/HTTPRoute/Backend/SessionPersistenceWeighted/DynamicTest/endpoint-inventory.tsv" <<'EOF'
initial	edgion-default	spw-a	30001	nonempty
initial	edgion-default	spw-b	30002	nonempty
zero_weight	edgion-default	spw-a	30001	nonempty
zero_weight	edgion-default	spw-b	30002	nonempty
final	edgion-default	spw-a	30001	nonempty
final	edgion-default	spw-b	30002	0
EOF
cat > "${FAKE_ENDPOINT_INVENTORY}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'INVENTORY:%s\n' "$*" >> "${KUBECTL_LOG}"
if [[ -n "${FAKE_ENDPOINT_FAILURE_MATCH:-}" && "$*" == *"${FAKE_ENDPOINT_FAILURE_MATCH}"* ]]; then
  exit 1
fi
case " $* " in
  *" --expected-count 0 "*) ;;
  *) printf '192.0.2.10:30001\n' ;;
esac
EOF
chmod +x "${FAKE_ENDPOINT_INVENTORY}"

cp -R "${SAFE_CONF}" "${FORBIDDEN_CONF}"
cat > "${FORBIDDEN_CONF}/local-endpointslice.yaml" <<'EOF'
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: forbidden-local-fixture
EOF

mkdir -p "${FAKE_BIN}"
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

printf 'CALL:%s\n' "$*" >> "${KUBECTL_LOG}"
if [[ "${FAKE_TERMINATE_FAILURE:-false}" == "true" && "$*" == *"--request-timeout=10s exec"* ]]; then
  exit 1
fi
if [[ "${FAKE_POD_DELETE_FAILURE:-false}" == "true" && "$*" == *"delete pod"* ]]; then
  exit 1
fi
case "$*" in
  *--print-build-capabilities*)
    printf 'body-buffering\n'
    ;;
  *--list-dynamic-scenarios*)
    printf '%s\n' "${FAKE_DYNAMIC_REGISTRY:-BasicAuthDynamic BasicAuthDynamicGrouped RateLimitLocalDynamic}" | tr ' ' '\n'
    ;;
  *"apply --server-side"*)
    printf 'RESET:%s\n' "$*" >> "${KUBECTL_LOG}"
    apply_target=""
    previous=""
    for argument in "$@"; do
      if [[ "${previous}" == "-f" ]]; then
        apply_target="${argument}"
        break
      fi
      previous="${argument}"
    done
    if [[ -n "${apply_target}" && -d "${apply_target}" ]]; then
      find "${apply_target}" -type f \( -name '*.yaml' -o -name '*.yml' \) -exec \
        awk '/image: / || /imagePullPolicy: / {print "RENDERED:" $0}' {} \; >> "${KUBECTL_LOG}"
    fi
    if [[ -n "${FAKE_RESET_FAILURE_MATCH:-}" && "$*" == *"${FAKE_RESET_FAILURE_MATCH}"* ]]; then
      exit 1
    fi
    ;;
  *"get edgionplugins.edgion.io"*)
    printf '%s' "${FAKE_LIVE_RESOURCE_VERSION:-101}"
    ;;
  *"get gatewayclasses.gateway.networking.k8s.io params-flap-class"*)
    printf '%s' "${FAKE_LIVE_RESOURCE_VERSION:-101}"
    ;;
  *"get httproutes.gateway.networking.k8s.io -n edgion-default spw-route"*)
    printf '%s' "${FAKE_LIVE_RESOURCE_VERSION:-101}"
    ;;
  *"/configclient/edgionplugins/list"*)
    gateway_rv="${FAKE_GATEWAY_RESOURCE_VERSION:-${FAKE_LIVE_RESOURCE_VERSION:-101}}"
    printf '{"success":true,"count":3,"data":['
    printf '{"metadata":{"namespace":"edgion-default","name":"basic-auth-dynamic","resourceVersion":"%s"}},' "${gateway_rv}"
    printf '{"metadata":{"namespace":"edgion-default","name":"basic-auth-dynamic-grouped","resourceVersion":"%s"}},' "${gateway_rv}"
    printf '{"metadata":{"namespace":"edgion-default","name":"rate-limit-dynamic","resourceVersion":"%s"}}' "${gateway_rv}"
    printf ']}\n'
    ;;
  *"/configclient/gatewayclass/list"*)
    gateway_rv="${FAKE_GATEWAY_RESOURCE_VERSION:-${FAKE_LIVE_RESOURCE_VERSION:-101}}"
    printf '{"success":true,"count":1,"data":['
    printf '{"metadata":{"name":"params-flap-class","resourceVersion":"%s"}}' "${gateway_rv}"
    printf ']}\n'
    ;;
  *"/configclient/httproute/list"*)
    gateway_rv="${FAKE_GATEWAY_RESOURCE_VERSION:-${FAKE_LIVE_RESOURCE_VERSION:-101}}"
    printf '{"success":true,"count":1,"data":['
    printf '{"metadata":{"namespace":"edgion-default","name":"spw-route","resourceVersion":"%s"}}' "${gateway_rv}"
    printf ']}\n'
    ;;
  *"tar -xf -"*)
    tar -tf - | sed 's|^|STAGED:|' >> "${KUBECTL_LOG}"
    ;;
  *"find "*"._"*)
    if [[ "${FAKE_STAGED_METADATA:-false}" == "true" ]]; then
      echo "/tmp/staged/._resource.yaml"
    fi
    ;;
  *"cat >"*)
    cat >/dev/null
    printf 'PORTS_STAGED\n' >> "${KUBECTL_LOG}"
    ;;
  *--dynamic-scenario*)
    scenario_id=""
    for argument in "$@"; do
      case "${argument}" in
        BasicAuthDynamic|BasicAuthDynamicGrouped|RateLimitLocalDynamic|GcParamsFlap|SpWeighted)
          scenario_id="${argument}"
          ;;
      esac
    done
    printf 'RUN:%s\n' "${scenario_id}" >> "${KUBECTL_LOG}"
    if [[ "${scenario_id}" == "SpWeighted" ]]; then
      for phase in initial zero_weight final; do
        printf '@@DYNAMIC_PHASE SpWeighted %s\n' "${phase}"
        if [[ "${FAKE_DYNAMIC_HANG:-false}" == "true" ]]; then
          while true; do sleep 1; done
        fi
        IFS= read -r acknowledgement
        printf 'ACK:%s\n' "${acknowledgement}" >> "${KUBECTL_LOG}"
        [[ "${acknowledgement}" == "@@DYNAMIC_PHASE_ACK SpWeighted ${phase} OK" ]] || exit 9
      done
    fi
    if [[ "${FAKE_DYNAMIC_FAILURE:-}" == "${scenario_id}" ]]; then
      exit 7
    fi
    ;;
esac
EOF
chmod +x "${FAKE_BIN}/kubectl"

cat > "${FAKE_BIN}/tar" <<EOF
#!/usr/bin/env bash
set -euo pipefail
printf 'TAR_ARGS:%s\n' "\$*" >> "\${KUBECTL_LOG}"
exec "${SYSTEM_TAR}" "\$@"
EOF
chmod +x "${FAKE_BIN}/tar"

: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  "${RUNNER}" --conf-root "${FORBIDDEN_CONF}" --scenario BasicAuthDynamic \
  > "${TEMP_DIR}/forbidden.log" 2>&1; then
  fail "EndpointSlice fixture was accepted"
fi
assert_contains "${TEMP_DIR}/forbidden.log" "Endpoint/EndpointSlice kind found"
if [[ -s "${KUBECTL_LOG}" ]]; then
  fail "kubectl was called before EndpointSlice validation failed"
fi

: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario UnsupportedDynamic \
  > "${TEMP_DIR}/unsupported.log" 2>&1; then
  fail "unsupported dynamic scenario was accepted"
fi
assert_contains "${TEMP_DIR}/unsupported.log" "unsupported K8s dynamic scenario"
if [[ -s "${KUBECTL_LOG}" ]]; then
  fail "kubectl was called for an unsupported scenario"
fi

: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  FAKE_DYNAMIC_REGISTRY="BasicAuthDynamic BasicAuthDynamicGrouped" \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario RateLimitLocalDynamic \
  > "${TEMP_DIR}/missing-registry.log" 2>&1; then
  fail "scenario missing from the test-client registry was accepted"
fi
assert_contains "${TEMP_DIR}/missing-registry.log" "dynamic registry does not contain RateLimitLocalDynamic"
assert_not_contains "${KUBECTL_LOG}" "STAGED:"
assert_not_contains "${KUBECTL_LOG}" "RUN:"

: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  FAKE_STAGED_METADATA=true \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario BasicAuthDynamic \
  > "${TEMP_DIR}/metadata.log" 2>&1; then
  fail "staged macOS archive metadata was accepted"
fi
assert_contains "${TEMP_DIR}/metadata.log" "macOS archive metadata was staged"
assert_not_contains "${KUBECTL_LOG}" "RUN:"

: > "${KUBECTL_LOG}"
PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  SYNC_TIMEOUT_SECONDS=2 SYNC_POLL_INTERVAL_SECONDS=1 \
  "${RUNNER}" --conf-root "${SAFE_CONF}" > "${TEMP_DIR}/success.log" 2>&1 \
  || fail "supported K8s dynamic scenarios did not run"
assert_contains "${TEMP_DIR}/success.log" "@@DYNAMIC_RESULT BasicAuthDynamic PASS 0"
assert_contains "${TEMP_DIR}/success.log" "@@DYNAMIC_RESULT BasicAuthDynamicGrouped PASS 0"
assert_contains "${TEMP_DIR}/success.log" "@@DYNAMIC_RESULT RateLimitLocalDynamic PASS 0"
assert_contains "${TEMP_DIR}/success.log" "@@DYNAMIC_SUMMARY total=3 failed=0"
assert_contains "${KUBECTL_LOG}" "STAGED:./EdgionPlugins/BasicAuth/DynamicTest/initial/resource.yaml"
assert_contains "${KUBECTL_LOG}" "STAGED:./EdgionPlugins/RateLimitLocal/DynamicTest/initial/resource.yaml"
assert_contains "${KUBECTL_LOG}" "TAR_ARGS:--no-xattrs --exclude=._* --exclude=__MACOSX"
assert_contains "${KUBECTL_LOG}" "RUN:BasicAuthDynamic"
assert_contains "${KUBECTL_LOG}" "EDGION_TEST_GATEWAY_CAPABILITIES=body-buffering"
assert_contains "${KUBECTL_LOG}" "RUN:BasicAuthDynamicGrouped"
assert_contains "${KUBECTL_LOG}" "RUN:RateLimitLocalDynamic"
assert_not_contains "${KUBECTL_LOG}" "Endpoint"

[[ "$(grep -c 'RESET:.*BasicAuth/DynamicTest/initial$' "${KUBECTL_LOG}")" -eq 2 ]] \
  || fail "BasicAuthDynamic was not reset before and after execution"
[[ "$(grep -c 'RESET:.*BasicAuth/DynamicTest/initial_grouped$' "${KUBECTL_LOG}")" -eq 2 ]] \
  || fail "BasicAuthDynamicGrouped was not reset before and after execution"
[[ "$(grep -c 'RESET:.*RateLimitLocal/DynamicTest/initial$' "${KUBECTL_LOG}")" -eq 2 ]] \
  || fail "RateLimitLocalDynamic was not reset before and after execution"

: > "${KUBECTL_LOG}"
for repeat_run in 1 2; do
  PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
    SYNC_TIMEOUT_SECONDS=2 SYNC_POLL_INTERVAL_SECONDS=1 \
    "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario RateLimitLocalDynamic \
    > "${TEMP_DIR}/repeat-${repeat_run}.log" 2>&1 \
    || fail "repeat run ${repeat_run} failed"
done
[[ "$(grep -c 'RESET:.*RateLimitLocal/DynamicTest/initial$' "${KUBECTL_LOG}")" -eq 4 ]] \
  || fail "repeat runs did not each reset before and after execution"
[[ "$(grep -c 'RUN:RateLimitLocalDynamic' "${KUBECTL_LOG}")" -eq 2 ]] \
  || fail "repeat runs did not each execute the scenario"

: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  SYNC_TIMEOUT_SECONDS=1 SYNC_POLL_INTERVAL_SECONDS=1 \
  FAKE_GATEWAY_RESOURCE_VERSION=100 \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario BasicAuthDynamic \
  > "${TEMP_DIR}/stale-gateway.log" 2>&1; then
  fail "stale Gateway resourceVersion was accepted"
fi
assert_contains "${TEMP_DIR}/stale-gateway.log" "Gateway did not converge for BasicAuthDynamic"
assert_contains "${TEMP_DIR}/stale-gateway.log" "@@DYNAMIC_RESULT BasicAuthDynamic FAIL 1"
assert_not_contains "${KUBECTL_LOG}" "RUN:"

: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  SYNC_TIMEOUT_SECONDS=2 SYNC_POLL_INTERVAL_SECONDS=1 \
  FAKE_DYNAMIC_FAILURE=BasicAuthDynamic \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario BasicAuthDynamic \
  > "${TEMP_DIR}/scenario-failure.log" 2>&1; then
  fail "failed dynamic scenario returned success"
fi
assert_contains "${TEMP_DIR}/scenario-failure.log" "@@DYNAMIC_RESULT BasicAuthDynamic FAIL 7"
[[ "$(grep -c 'RESET:.*BasicAuth/DynamicTest/initial$' "${KUBECTL_LOG}")" -eq 2 ]] \
  || fail "failed scenario was not restored after execution"

: > "${KUBECTL_LOG}"
PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  DYNAMIC_MANIFEST="${CLUSTER_DYNAMIC_MANIFEST}" FAKE_DYNAMIC_REGISTRY="GcParamsFlap" \
  SYNC_TIMEOUT_SECONDS=2 SYNC_POLL_INTERVAL_SECONDS=1 \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario GcParamsFlap \
  > "${TEMP_DIR}/cluster-scoped.log" 2>&1 \
  || fail "cluster-scoped dynamic scenario did not run"
assert_contains "${TEMP_DIR}/cluster-scoped.log" "@@DYNAMIC_RESULT GcParamsFlap PASS 0"
[[ "$(grep -c 'CALL:delete edgiongatewayclassconfigs.edgion.io params-flap-egc --ignore-not-found --wait=true' "${KUBECTL_LOG}")" -eq 2 ]] \
  || fail "reset-absent cluster resource was not deleted before and after execution"
assert_not_contains "${KUBECTL_LOG}" "get gatewayclasses.gateway.networking.k8s.io -n -"

: > "${KUBECTL_LOG}"
PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  DYNAMIC_MANIFEST="${ENDPOINT_DYNAMIC_MANIFEST}" ENDPOINT_INVENTORY="${FAKE_ENDPOINT_INVENTORY}" \
  TEST_SERVER_IMAGE="registry.test/test-server:current" PULL_POLICY_OVERRIDE=Never \
  FAKE_DYNAMIC_REGISTRY="SpWeighted" SYNC_TIMEOUT_SECONDS=2 SYNC_POLL_INTERVAL_SECONDS=1 \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario SpWeighted \
  > "${TEMP_DIR}/endpoint-phases.log" 2>&1 \
  || fail "EndpointSlice phase barriers did not run"
assert_contains "${TEMP_DIR}/endpoint-phases.log" "@@DYNAMIC_RESULT SpWeighted PASS 0"
[[ "$(grep -c '^INVENTORY:' "${KUBECTL_LOG}")" -eq 6 ]] \
  || fail "live EndpointSlice inventory was not re-read for every phase and Service"
[[ "$(grep -c '^ACK:@@DYNAMIC_PHASE_ACK SpWeighted ' "${KUBECTL_LOG}")" -eq 3 ]] \
  || fail "test-client did not receive one EndpointSlice acknowledgement per phase"
assert_contains "${KUBECTL_LOG}" "INVENTORY:--namespace edgion-default --service spw-b --port 30002"
assert_contains "${KUBECTL_LOG}" "--expected-count 0"
assert_contains "${KUBECTL_LOG}" "RENDERED:        image: registry.test/test-server:current"
assert_contains "${KUBECTL_LOG}" "RENDERED:        imagePullPolicy: Never"

: > "${KUBECTL_LOG}"
cp "${SAFE_CONF}/HTTPRoute/Backend/SessionPersistenceWeighted/DynamicTest/endpoint-inventory.tsv" \
  "${TEMP_DIR}/endpoint-inventory.tsv.saved"
cat > "${SAFE_CONF}/HTTPRoute/Backend/SessionPersistenceWeighted/DynamicTest/endpoint-inventory.tsv" <<'EOF'
initial	edgion-default	spw-a	app	nonempty
zero_weight	edgion-default	spw-a	30001	nonempty
final	edgion-default	spw-a	30001	nonempty
EOF
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  DYNAMIC_MANIFEST="${ENDPOINT_DYNAMIC_MANIFEST}" ENDPOINT_INVENTORY="${FAKE_ENDPOINT_INVENTORY}" \
  FAKE_DYNAMIC_REGISTRY="SpWeighted" SYNC_TIMEOUT_SECONDS=2 SYNC_POLL_INTERVAL_SECONDS=1 \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario SpWeighted \
  > "${TEMP_DIR}/named-endpoint-port.log" 2>&1; then
  fail "named EndpointSlice inventory port was accepted"
fi
mv "${TEMP_DIR}/endpoint-inventory.tsv.saved" \
  "${SAFE_CONF}/HTTPRoute/Backend/SessionPersistenceWeighted/DynamicTest/endpoint-inventory.tsv"
assert_contains "${TEMP_DIR}/named-endpoint-port.log" \
  "endpoint inventory port must be a numeric port for SpWeighted/initial"
assert_not_contains "${KUBECTL_LOG}" "INVENTORY:"

: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  DYNAMIC_MANIFEST="${ENDPOINT_DYNAMIC_MANIFEST}" ENDPOINT_INVENTORY="${FAKE_ENDPOINT_INVENTORY}" \
  FAKE_DYNAMIC_REGISTRY="SpWeighted" FAKE_DYNAMIC_HANG=true \
  SYNC_TIMEOUT_SECONDS=2 SYNC_POLL_INTERVAL_SECONDS=1 PHASE_EXEC_TIMEOUT_SECONDS=2 \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario SpWeighted \
  > "${TEMP_DIR}/phase-timeout.log" 2>&1; then
  fail "hung dynamic phase execution returned success"
fi
assert_contains "${TEMP_DIR}/phase-timeout.log" "dynamic scenario SpWeighted exceeded 2s execution deadline"
assert_contains "${TEMP_DIR}/phase-timeout.log" "@@DYNAMIC_RESULT SpWeighted FAIL 124"
assert_contains "${KUBECTL_LOG}" "CALL:--request-timeout=10s exec"
assert_contains "${KUBECTL_LOG}" "phase-process.pid"
[[ "$(grep -c 'RESET:.*SessionPersistenceWeighted/DynamicTest/initial$' "${KUBECTL_LOG}")" -eq 2 ]] \
  || fail "timed-out dynamic phase was not restored after execution"

: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  DYNAMIC_MANIFEST="${ENDPOINT_DYNAMIC_MANIFEST}" ENDPOINT_INVENTORY="${FAKE_ENDPOINT_INVENTORY}" \
  FAKE_DYNAMIC_REGISTRY="SpWeighted" FAKE_DYNAMIC_HANG=true \
  FAKE_TERMINATE_FAILURE=true FAKE_POD_DELETE_FAILURE=true \
  SYNC_TIMEOUT_SECONDS=2 SYNC_POLL_INTERVAL_SECONDS=1 PHASE_EXEC_TIMEOUT_SECONDS=2 \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario SpWeighted \
  > "${TEMP_DIR}/containment-failure.log" 2>&1; then
  fail "uncontained remote dynamic process returned success"
fi
assert_contains "${TEMP_DIR}/containment-failure.log" "remote dynamic process containment failed"
assert_contains "${TEMP_DIR}/containment-failure.log" "skipping fixture restoration"
assert_contains "${TEMP_DIR}/containment-failure.log" "@@DYNAMIC_RESULT SpWeighted FAIL 125"
[[ "$(grep -c 'RESET:.*SessionPersistenceWeighted/DynamicTest/initial$' "${KUBECTL_LOG}")" -eq 1 ]] \
  || fail "uncontained remote dynamic process restored fixtures unsafely"
watchdog_cleanup_line="$(grep -nF 'kill "${PHASE_WATCHDOG_PID}"' "${RUNNER}" | tail -n 1 | cut -d: -f1)"
remote_cleanup_line="$(grep -nF 'if contain_remote_phase_process >/dev/null 2>&1; then' "${RUNNER}" | tail -n 1 | cut -d: -f1)"
[[ -n "${watchdog_cleanup_line}" && -n "${remote_cleanup_line}" \
  && "${watchdog_cleanup_line}" -lt "${remote_cleanup_line}" ]] \
  || fail "signal cleanup does not serialize watchdog shutdown before remote containment"

: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" POD=fake-pod KUBECTL_LOG="${KUBECTL_LOG}" PORTS_CONFIG="${PORTS_CONFIG}" \
  DYNAMIC_MANIFEST="${ENDPOINT_DYNAMIC_MANIFEST}" ENDPOINT_INVENTORY="${FAKE_ENDPOINT_INVENTORY}" \
  FAKE_DYNAMIC_REGISTRY="SpWeighted" FAKE_ENDPOINT_FAILURE_MATCH="--service spw-b" \
  SYNC_TIMEOUT_SECONDS=2 SYNC_POLL_INTERVAL_SECONDS=1 \
  "${RUNNER}" --conf-root "${SAFE_CONF}" --scenario SpWeighted \
  > "${TEMP_DIR}/endpoint-failure.log" 2>&1; then
  fail "failed EndpointSlice phase inventory returned success"
fi
assert_contains "${TEMP_DIR}/endpoint-failure.log" "@@DYNAMIC_RESULT SpWeighted FAIL 9"
[[ "$(grep -c 'RESET:.*SessionPersistenceWeighted/DynamicTest/initial$' "${KUBECTL_LOG}")" -eq 2 ]] \
  || fail "EndpointSlice inventory failure did not restore the scenario"
[[ "$(grep -c '^INVENTORY:' "${KUBECTL_LOG}")" -eq 2 ]] \
  || fail "EndpointSlice inventory failure was retried or later phases were read"

echo "OK: K8s dynamic scenario runner regression tests passed"
