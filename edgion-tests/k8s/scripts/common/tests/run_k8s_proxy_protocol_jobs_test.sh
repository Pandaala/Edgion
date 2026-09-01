#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../run_k8s_proxy_protocol_jobs.sh"
CLIENT_SCRIPT="${SCRIPT_DIR}/../../../docker/run-k8s-proxy-protocol-test.sh"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

FAKE_BIN="${TEST_ROOT}/bin"
FAKE_KUBECTL="${FAKE_BIN}/kubectl"
MUTATIONS="${TEST_ROOT}/mutations.log"
mkdir -p "$FAKE_BIN"

cat > "$FAKE_KUBECTL" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

mode="${FAKE_MODE:?}"
mutations="${FAKE_MUTATIONS:?}"
command_name="${1:-}"
shift || true
joined=" $* "

scenario_for_args() {
  case "$joined" in
    *proxy_protocol_chain*|*'-chain '*) echo proxy_protocol_chain ;;
    *) echo proxy_protocol_inbound ;;
  esac
}

case "$command_name" in
  version)
    echo 'Client Version: v1.33.0'
    ;;
  create|label|apply)
    echo "${command_name} $*" >> "$mutations"
    ;;
  delete)
    echo "delete $*" >> "$mutations"
    [ "$mode" != cleanup_fail ]
    ;;
  get)
    resource="${1:-}"
    name="${2:-}"
    case "$resource" in
      nodes)
        printf '10.244.0.0/16\n'
        ;;
      service)
        printf '10.96.0.25'
        ;;
      deployment)
        printf '1'
        ;;
      job)
        if [[ "$joined" == *'jsonpath={.status.succeeded}'* ]]; then
          if [ "$name" = edgion-proxy-cert-bootstrap ]; then
            printf '1'
          else
            case "$mode" in
              success|missing_marker|duplicate_marker|wrong_id|bad_gateway_log|cleanup_fail) printf '1' ;;
              *) : ;;
            esac
          fi
        elif [[ "$joined" == *'jsonpath={.status.failed}'* ]]; then
          if [ "$name" = edgion-proxy-client ] && [ "$mode" = client_failure ]; then
            printf '1'
          fi
        elif [[ "$joined" == *'conditions'* ]]; then
          :
        else
          echo 'apiVersion: batch/v1'
        fi
        ;;
      pods)
        if [[ "$joined" == *'job-name=edgion-proxy-cert-bootstrap'* ]]; then
          printf 'pod-cert-bootstrap'
        elif [[ "$joined" == *'job-name=edgion-proxy-client'* ]]; then
          printf 'pod-client'
        elif [[ "$joined" == *'jsonpath={.items[*].metadata.name}'* ]]; then
          printf 'pod-controller pod-gateway pod-client'
        else
          echo 'NAME READY STATUS'
        fi
        ;;
      pod)
        if [[ "$joined" == *'terminated.exitCode'* ]]; then
          if [ "$name" = pod-client ] && [ "$mode" = client_failure ]; then printf '7'; else printf '0'; fi
        else
          echo 'apiVersion: v1'
        fi
        ;;
      all)
        echo 'apiVersion: v1'
        ;;
      events)
        echo 'No resources found'
        ;;
    esac
    ;;
  logs)
    target="${1:-}"
    case "$target" in
      job/edgion-proxy-cert-bootstrap)
        if [ "$mode" = bootstrap_missing ]; then
          echo 'certificate generator exited without a marker'
        else
          echo '@@ISOLATED_BOOTSTRAP_RESULT PASS 0'
        fi
        ;;
      job/edgion-proxy-client)
        scenario_id="$(scenario_for_args)"
        case "$mode" in
          client_failure) printf '@@ISOLATED_CONTAINER_RESULT %s FAIL 7\n' "$scenario_id" ;;
          missing_marker) echo 'client exited without a marker' ;;
          duplicate_marker)
            printf '@@ISOLATED_CONTAINER_RESULT %s PASS 0\n' "$scenario_id"
            printf '@@ISOLATED_CONTAINER_RESULT %s PASS 0\n' "$scenario_id"
            ;;
          wrong_id) echo '@@ISOLATED_CONTAINER_RESULT unknown_proxy PASS 0' ;;
          *) printf '@@ISOLATED_CONTAINER_RESULT %s PASS 0\n' "$scenario_id" ;;
        esac
        ;;
      deployment/edgion-proxy-gateway)
        if [ "$mode" = bad_gateway_log ]; then
          echo 'gateway started without listener evidence'
        else
          echo 'Inbound PROXY protocol enabled for listener'
        fi
        ;;
      *) echo 'fake component log' ;;
    esac
    ;;
  describe)
    echo 'fake pod description'
    ;;
  *)
    echo "unexpected fake kubectl command: ${command_name} $*" >&2
    exit 90
    ;;
esac
EOF
chmod +x "$FAKE_KUBECTL"

run_helper_case() {
  local mode="$1" scenario_id="$2" expected_rc="$3" expected_result="$4" expected_summary="$5"
  local case_dir="${TEST_ROOT}/${mode}-${scenario_id}"
  mkdir -p "$case_dir"
  : > "$MUTATIONS"
  set +e
  FAKE_MODE="$mode" FAKE_MUTATIONS="$MUTATIONS" KUBECTL_BIN="$FAKE_KUBECTL" \
    "$HELPER" \
      --image edgion-isolated:test \
      --haproxy-image haproxy:3.0-alpine \
      --job "$scenario_id" \
      --timeout-seconds 1 \
      --artifacts-dir "$case_dir" \
      > "${case_dir}/output.log" 2>&1
  actual_rc=$?
  set -e
  [ "$actual_rc" -eq "$expected_rc" ]
  grep -qx "$expected_result" "${case_dir}/output.log"
  grep -qx "$expected_summary" "${case_dir}/output.log"
  grep -q '^create namespace edgion-proxy-isolated-' "$MUTATIONS"
  grep -q '^delete namespace edgion-proxy-isolated-' "$MUTATIONS"
}

run_helper_case success proxy_protocol_inbound 0 \
  '@@ISOLATED_RESULT proxy_protocol_inbound PASS 0' \
  '@@ISOLATED_SUMMARY total=1 failed=0 infrastructure_failed=0'
! find "${TEST_ROOT}/success-proxy_protocol_inbound/proxy_protocol_inbound/conf" \
  -type f -name '*Endpoint*' | grep -q .
grep -q 'app.kubernetes.io/name: edgion-proxy-test-server' \
  "${TEST_ROOT}/success-proxy_protocol_inbound/proxy_protocol_inbound/conf/Service_edgion-test_ppin-http.yaml" \
  || { cat "${TEST_ROOT}/success-proxy_protocol_inbound/proxy_protocol_inbound/conf/Service_edgion-test_ppin-http.yaml"; exit 1; }
! rg -q '__[A-Z_]+__' "${TEST_ROOT}/success-proxy_protocol_inbound/proxy_protocol_inbound"/*.yaml

run_helper_case success proxy_protocol_chain 0 \
  '@@ISOLATED_RESULT proxy_protocol_chain PASS 0' \
  '@@ISOLATED_SUMMARY total=1 failed=0 infrastructure_failed=0'
! find "${TEST_ROOT}/success-proxy_protocol_chain/proxy_protocol_chain/conf" \
  -type f -name '*Endpoint*' | grep -q .
grep -q 'app.kubernetes.io/name: edgion-proxy-gateway' \
  "${TEST_ROOT}/success-proxy_protocol_chain/proxy_protocol_chain/conf/Service_edgion-test_chain-hop2.yaml"
grep -q '"10.244.0.0/16"' \
  "${TEST_ROOT}/success-proxy_protocol_chain/proxy_protocol_chain/conf/EdgionGatewayConfig__chain-egc.yaml"

run_helper_case client_failure proxy_protocol_inbound 1 \
  '@@ISOLATED_RESULT proxy_protocol_inbound FAIL 7' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_helper_case missing_marker proxy_protocol_inbound 1 \
  '@@ISOLATED_RESULT proxy_protocol_inbound FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_helper_case duplicate_marker proxy_protocol_chain 1 \
  '@@ISOLATED_RESULT proxy_protocol_chain FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_helper_case wrong_id proxy_protocol_inbound 1 \
  '@@ISOLATED_RESULT proxy_protocol_inbound FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_helper_case bootstrap_missing proxy_protocol_chain 1 \
  '@@ISOLATED_RESULT proxy_protocol_chain FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_helper_case bad_gateway_log proxy_protocol_chain 1 \
  '@@ISOLATED_RESULT proxy_protocol_chain FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_helper_case timeout proxy_protocol_chain 1 \
  '@@ISOLATED_RESULT proxy_protocol_chain FAIL 124' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_helper_case cleanup_fail proxy_protocol_inbound 1 \
  '@@ISOLATED_RESULT proxy_protocol_inbound PASS 0' \
  '@@ISOLATED_SUMMARY total=1 failed=0 infrastructure_failed=1'

: > "$MUTATIONS"
set +e
FAKE_MODE=success FAKE_MUTATIONS="$MUTATIONS" KUBECTL_BIN="$FAKE_KUBECTL" \
  "$HELPER" --image edgion-isolated:test --job unsupported_proxy \
  --artifacts-dir "${TEST_ROOT}/unsupported" > "${TEST_ROOT}/unsupported.log" 2>&1
unsupported_rc=$?
set -e
[ "$unsupported_rc" -eq 2 ]
[ ! -s "$MUTATIONS" ]

cat > "${FAKE_BIN}/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

joined=" $* "
url="${!#}"
pod_ip="${POD_IP:?}"
identity="$pod_ip"
if [ "${FAKE_BAD_IDENTITY:-0}" = 1 ]; then identity=10.244.99.99; fi

case "$url" in
  http://edgion-controller:12153/ready|http://edgion-gateway:12161/health)
    exit 0
    ;;
  http://edgion-wire-dump:18080/v2)
    printf '0d0a0d0a000d0a515549540a2100000c'
    ;;
  http://edgion-wire-dump:18080/v1)
    printf '50524f585920544350340d0a'
    ;;
  http://edgion-haproxy:58442/|http://edgion-haproxy:58443/|http://edgion-haproxy:58542/)
    exit 52
    ;;
  http://edgion-haproxy:58440/headers|http://edgion-haproxy:58441/headers)
    printf '{"headers":{"x-forwarded-for":"%s","x-real-ip":"%s"}}' "$identity" "$identity"
    ;;
  http://edgion-haproxy:58540/headers)
    printf '{"headers":{"x-forwarded-for":"%s, 127.0.0.1","x-edgion-remote-ip":"%s"}}' "$identity" "$identity"
    ;;
  http://edgion-gateway:58430/headers)
    if [[ "$joined" == *" -w "* ]]; then printf '000'; fi
    exit 52
    ;;
  http://edgion-gateway:58431/headers)
    printf '{"headers":{"x-forwarded-for":"%s"}}' "$identity"
    ;;
  *)
    echo "unexpected fake curl URL: ${url}" >&2
    exit 90
    ;;
esac
EOF
cat > "${FAKE_BIN}/sleep" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "${FAKE_BIN}/curl" "${FAKE_BIN}/sleep"

for scenario_id in proxy_protocol_inbound proxy_protocol_chain; do
  POD_IP=10.244.1.23 PATH="${FAKE_BIN}:$PATH" \
    "$CLIENT_SCRIPT" "$scenario_id" > "${TEST_ROOT}/client-${scenario_id}.log" 2>&1
  grep -qx "@@ISOLATED_CONTAINER_RESULT ${scenario_id} PASS 0" \
    "${TEST_ROOT}/client-${scenario_id}.log"
done

set +e
POD_IP=10.244.1.23 FAKE_BAD_IDENTITY=1 PATH="${FAKE_BIN}:$PATH" \
  "$CLIENT_SCRIPT" proxy_protocol_chain > "${TEST_ROOT}/client-bad-identity.log" 2>&1
bad_identity_rc=$?
set -e
[ "$bad_identity_rc" -eq 1 ]
grep -qx '@@ISOLATED_CONTAINER_RESULT proxy_protocol_chain FAIL 1' \
  "${TEST_ROOT}/client-bad-identity.log"

echo 'run_k8s_proxy_protocol_jobs_test.sh: PASS'
