#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_RUNNER="${SCRIPT_DIR}/../../run_k8s_integration.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-k8s-image-override-test.XXXXXX)"
FUNCTION_FILE="${TEMP_DIR}/override-function.sh"
FAKE_BIN="${TEMP_DIR}/bin"
KUBECTL_LOG="${TEMP_DIR}/kubectl.log"

cleanup() {
  rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT INT TERM

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

awk '
  /^wait_for_test_server_image_rollout\(\) \{/ { emit = 1 }
  /^restart_secret_dependent_workloads\(\) \{/ { emit = 0 }
  emit { print }
' "${MAIN_RUNNER}" > "${FUNCTION_FILE}"
grep -Fq 'override_suite_test_server_images()' "${FUNCTION_FILE}" \
  || fail "could not extract image override function"

mkdir -p "${FAKE_BIN}"
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${KUBECTL_LOG}"
case "$*" in
  "--request-timeout=10s get deployment "*)
    count=0
    [[ ! -f "${TEMP_DIR}/read-count" ]] || count="$(cat "${TEMP_DIR}/read-count")"
    count=$((count + 1))
    printf '%s\n' "${count}" > "${TEMP_DIR}/read-count"
    jq -cn --arg mode "${ROLLOUT_MODE:-healthy}" --argjson count "${count}" '
      {metadata:{uid:"deployment-uid",generation:27},spec:{replicas:1,template:{spec:{containers:[{name:"test-server",image:"registry.test/test-server:current"}]}}},
       status:{observedGeneration:27,updatedReplicas:1,readyReplicas:1,availableReplicas:1,replicas:1,conditions:[{type:"Available",status:"True"},{type:"Progressing",status:"True",reason:"NewReplicaSetAvailable"}]}}
      | if $mode == "stale" and $count == 1 or $mode == "persistent" then
          .status.conditions[1] = {type:"Progressing",status:"False",reason:"ProgressDeadlineExceeded"}
        elif $mode == "old-generation" then .status.observedGeneration = 26
        elif $mode == "wrong-image" then .spec.template.spec.containers[0].image = "registry.test/test-server:old"
        elif $mode == "replaced" and $count > 1 then .metadata.uid = "replacement-uid"
        elif $mode == "replaced" then .status.readyReplicas = 0
        elif $mode == "api-failure" then empty
        else . end'
    [[ "${ROLLOUT_MODE:-healthy}" != api-failure ]] || exit 1
    ;;
  "get deployment lb-multi-test-server"*) exit 1 ;;
  "get deployments --all-namespaces -l edgion.io/test-server-image=true -o json")
    printf '{"items":[{"metadata":{"namespace":"edgion-test","name":"ordinary-dynamic","labels":{}}},{"metadata":{"namespace":"edgion-test","name":"direct-port-unready","labels":{"edgion.io/test-server-rollout-ready":"false"}}}]}\n'
    ;;
  "get pods -n edgion-test -l app=direct-port-unready -o json")
    printf '{"items":[{"metadata":{},"status":{"phase":"Running"},"spec":{"containers":[{"name":"test-server","image":"registry.test/test-server:current"}]}}]}\n'
    ;;
esac
EOF
chmod +x "${FAKE_BIN}/kubectl"
cat > "${FAKE_BIN}/date" <<'EOF'
#!/usr/bin/env bash
count=0
[[ ! -f "${TEMP_DIR}/clock-count" ]] || count="$(cat "${TEMP_DIR}/clock-count")"
printf '%s\n' "$((count + 1))" > "${TEMP_DIR}/clock-count"
printf '%s\n' "$((count * 100))"
EOF
cat > "${FAKE_BIN}/sleep" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "${FAKE_BIN}/date" "${FAKE_BIN}/sleep"

SKIP_DEPLOY=false
TEST_SERVER_IMAGE=registry.test/test-server:current
PULL_POLICY_OVERRIDE=Never
export KUBECTL_LOG TEMP_DIR

# shellcheck source=/dev/null
source "${FUNCTION_FILE}"
PATH="${FAKE_BIN}:${PATH}" override_suite_test_server_images

grep -Fq 'get deployment ordinary-dynamic -n edgion-test -o json' "${KUBECTL_LOG}" \
  || fail "ordinary dynamic Deployment did not wait for rollout readiness"
grep -Fq 'get pods -n edgion-test -l app=direct-port-unready -o json' "${KUBECTL_LOG}" \
  || fail "intentionally unready Deployment did not inspect its Running Pod"
if grep -Fq 'rollout status deployment/direct-port-unready' "${KUBECTL_LOG}"; then
  fail "intentionally unready Deployment waited for rollout readiness"
fi

echo "OK: suite image overrides preserve intentionally unready Pods"

rm -f "${TEMP_DIR}/read-count" "${TEMP_DIR}/clock-count"
: > "${KUBECTL_LOG}"
export ROLLOUT_MODE=stale
PATH="${FAKE_BIN}:${PATH}" override_suite_test_server_images
[[ "$(cat "${TEMP_DIR}/read-count")" == 2 ]] || fail "stale rollout did not converge by observation"
[[ "$(grep -c '^set image deployment/ordinary-dynamic ' "${KUBECTL_LOG}")" == 1 ]] \
  || fail "stale rollout replayed its image mutation"

for ROLLOUT_MODE in persistent old-generation wrong-image replaced api-failure; do
  export ROLLOUT_MODE
  rm -f "${TEMP_DIR}/read-count" "${TEMP_DIR}/clock-count"
  if PATH="${FAKE_BIN}:${PATH}" wait_for_test_server_image_rollout edgion-test ordinary-dynamic; then
    fail "invalid rollout accepted: ${ROLLOUT_MODE}"
  fi
done
echo "OK: current image rollout converges without mutation replay and rejects stale or changed identities"
