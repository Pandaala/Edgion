#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_RUNNER="${SCRIPT_DIR}/../../run_k8s_integration.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-k8s-gateway-probe-test.XXXXXX)"
FAKE_BIN="${TEMP_DIR}/bin"

cleanup() {
  rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT INT TERM

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

mkdir -p "${FAKE_BIN}"
printf '0\n' > "${TEMP_DIR}/get-count"
printf '0\n' > "${TEMP_DIR}/forward-count"
printf '0\n' > "${TEMP_DIR}/curl-count"

cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

increment_file() {
  local file="$1"
  local value
  value="$(cat "${file}")"
  value=$((value + 1))
  printf '%s\n' "${value}" > "${file}"
  printf '%s\n' "${value}"
}

case "$*" in
  *"get pods"*)
    count="$(increment_file "${PROBE_TEMP_DIR}/get-count")"
    if [[ "${FAKE_NO_POD:-false}" != "true" && "${count}" -ge 2 ]]; then
      printf 'fake-gateway-pod'
    fi
    ;;
  *"port-forward"*)
    count="$(increment_file "${PROBE_TEMP_DIR}/forward-count")"
    if [[ "${count}" -eq 1 ]]; then
      echo "transient port-forward failure" >&2
      exit 1
    fi
    # Ensure the first polling attempt observes an empty log. This catches
    # loops that accidentally depend on newline word splitting under the
    # caller's tab-only IFS.
    sleep 0.1
    echo "Forwarding from 127.0.0.1:54321 -> 12001"
    trap 'exit 0' TERM INT
    while true; do
      sleep 1
    done
    ;;
  *)
    echo "unexpected kubectl invocation: $*" >&2
    exit 1
    ;;
esac
EOF

cat > "${FAKE_BIN}/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
count="$(cat "${PROBE_TEMP_DIR}/curl-count")"
count=$((count + 1))
printf '%s\n' "${count}" > "${PROBE_TEMP_DIR}/curl-count"
if [[ "${count}" -eq 1 ]]; then
  printf '%s\n' '{"success":true,"data":{"server_id":"epoch-1","ready":false}}'
else
  printf '%s\n' '{"success":true,"data":{"server_id":"epoch-1","ready":true}}'
fi
EOF
chmod +x "${FAKE_BIN}/kubectl" "${FAKE_BIN}/curl"

functions="$(sed -n '/^get_gateway_server_info() {/,/^get_gateway_pod_uid() {/p' "${MAIN_RUNNER}" | sed '$d')"
[[ -n "${functions}" ]] || fail "could not extract Gateway server-info probe functions"
eval "${functions}"

export PROBE_TEMP_DIR="${TEMP_DIR}"
GATEWAY_ADMIN_TOKEN="test-token"
GATEWAY_SYNC_READY_TIMEOUT_SECONDS=8
server_id=""
gateway_ready=""
IFS=$'\t' read -r server_id gateway_ready \
  <<< "$(PATH="${FAKE_BIN}:${PATH}" get_gateway_server_info 2> "${TEMP_DIR}/probe.err")"
[[ "${server_id}" == "epoch-1" && "${gateway_ready}" == "true" ]] \
  || fail "transient Pod/port-forward/readiness states did not converge: server_id=${server_id} ready=${gateway_ready}"
[[ "$(cat "${TEMP_DIR}/get-count")" -ge 2 ]] \
  || fail "Gateway Pod discovery was not retried"
[[ "$(cat "${TEMP_DIR}/forward-count")" -ge 2 ]] \
  || fail "failed port-forward was not retried"
[[ "$(cat "${TEMP_DIR}/curl-count")" -ge 2 ]] \
  || fail "not-ready server-info response was not retried"

GATEWAY_SYNC_READY_TIMEOUT_SECONDS=1
if output="$(PATH="${FAKE_BIN}:${PATH}" FAKE_NO_POD=true get_gateway_server_info 2> "${TEMP_DIR}/timeout.err")"; then
  :
fi
[[ "${output}" == $'unknown\tfalse' ]] \
  || fail "bounded no-Pod timeout returned an unexpected state: ${output}"
grep -Fq 'Gateway server-info probe timed out after 1s' "${TEMP_DIR}/timeout.err" \
  || fail "bounded no-Pod timeout lacked a diagnostic"
grep -Fq 'no running Gateway Pod' "${TEMP_DIR}/timeout.err" \
  || fail "bounded no-Pod timeout lost the last probe error"

echo "OK: Gateway server-info probe retries transient rollout states"
