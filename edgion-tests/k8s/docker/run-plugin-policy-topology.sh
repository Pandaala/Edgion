#!/usr/bin/env bash
# Run the unchanged policy lifecycle in an invocation-owned filesystem topology.
set -euo pipefail

canonical_script="${1:?canonical script is required}"
workspace="/workspace"
export EDGION_WORK_DIR
EDGION_WORK_DIR="$(mktemp -d "${workspace}/tmp/integration_testing/plugin-policy.XXXXXX")"
config_dir="${EDGION_WORK_DIR}/config"
log_dir="${EDGION_WORK_DIR}/logs"
pid_dir="${EDGION_WORK_DIR}/pids"
owned_pids=()
mkdir -p "${config_dir}/crd" "${log_dir}" "${pid_dir}" "${EDGION_WORK_DIR}/certs"

cleanup() {
  local pid deadline
  trap - EXIT
  for pid in ${owned_pids[@]+"${owned_pids[@]}"}; do
    kill -TERM "${pid}" 2>/dev/null || true
  done
  deadline=$((SECONDS + 8))
  for pid in ${owned_pids[@]+"${owned_pids[@]}"}; do
    while kill -0 "${pid}" 2>/dev/null && [[ "${SECONDS}" -lt "${deadline}" ]]; do
      sleep 0.1
    done
    kill -KILL "${pid}" 2>/dev/null || true
    wait "${pid}" 2>/dev/null || true
  done
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

wait_ready() {
  local pid="$1" url="$2" deadline=$((SECONDS + 60))
  while [[ "${SECONDS}" -lt "${deadline}" ]]; do
    kill -0 "${pid}" 2>/dev/null || return 1
    if curl --silent --fail --max-time 1 "${url}" >/dev/null; then
      return 0
    fi
    sleep 0.2
  done
  echo "ERROR: policy topology readiness deadline exceeded" >&2
  return 1
}

for binary in edgion-controller edgion-gateway test_server test_client; do
  [[ -x "${workspace}/target/debug/${binary}" ]] || {
    echo "ERROR: policy topology binary is missing: ${binary}" >&2
    exit 1
  }
done
cp -R "${workspace}/config/crd/." "${config_dir}/crd/"
cp "${workspace}/edgion-tests/integration/conf/PluginPolicy/DynamicTest/initial/"*.yaml "${config_dir}/"

# The canonical fixture uses this Secret for its preloaded TLS listeners.
openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
  -subj '/CN=pp-a.example.com' \
  -addext 'subjectAltName=DNS:pp-a.example.com,DNS:pp-b.example.com,IP:127.0.0.1' \
  -keyout "${EDGION_WORK_DIR}/certs/server.key" \
  -out "${EDGION_WORK_DIR}/certs/server.crt" >"${log_dir}/certificate.log" 2>&1
python3 - "${EDGION_WORK_DIR}" <<'PY'
import base64
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
cert = base64.b64encode((root / "certs/server.crt").read_bytes()).decode("ascii")
key = base64.b64encode((root / "certs/server.key").read_bytes()).decode("ascii")
(root / "config/Secret_edge-tls.yaml").write_text(
    "apiVersion: v1\nkind: Secret\nmetadata:\n  name: edge-tls\n  namespace: edgion-test\n"
    f"type: kubernetes.io/tls\ndata:\n  tls.crt: {cert}\n  tls.key: {key}\n",
    encoding="utf-8",
)
PY

# shellcheck source=../../integration/scripts/utils/conf_sync_certs.sh
source "${workspace}/edgion-tests/integration/scripts/utils/conf_sync_certs.sh"
conf_sync_distribute_certs "${EDGION_WORK_DIR}" >"${log_dir}/conf-sync-certificates.log" 2>&1
cat >"${EDGION_WORK_DIR}/controller.yaml" <<EOF
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: ${config_dir}
EOF
conf_sync_security_block server >>"${EDGION_WORK_DIR}/controller.yaml"
cat >"${EDGION_WORK_DIR}/gateway.yaml" <<EOF
work_dir: "."
logging:
  log_dir: "${log_dir}"
metrics:
  listen: "127.0.0.1:12090"
EOF
conf_sync_security_block client >>"${EDGION_WORK_DIR}/gateway.yaml"

export EDGION_ADMIN_TOKEN=integration-controller-admin-token-0123456789
export EDGION_TOKEN="${EDGION_ADMIN_TOKEN}"
export EDGION_TEST_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN}"
export EDGION_GATEWAY_ADMIN_TOKEN=integration-gateway-admin-token-0123456789
export EDGION_TEST_ADMIN_API_URL=http://127.0.0.1:12101
export EDGION_TEST_GATEWAY_ADMIN_URL=http://127.0.0.1:12001
export EDGION_ALLOW_INTEGRATION_TESTING_MODE=1
unset EDGION_TEST_K8S_MODE
EDGION_TEST_GATEWAY_CAPABILITIES="$("${workspace}/target/debug/edgion-gateway" --print-build-capabilities)"
export EDGION_TEST_GATEWAY_CAPABILITIES

"${workspace}/target/debug/test_server" --http-ports=30001 \
  --tcp-port=30010 --udp-port=30011 --https-backend-port=30051 \
  --cert-file="${EDGION_WORK_DIR}/certs/server.crt" \
  --key-file="${EDGION_WORK_DIR}/certs/server.key" \
  >"${log_dir}/test-server.log" 2>&1 &
server_pid=$!
owned_pids+=("${server_pid}")
printf '%s\n' "${server_pid}" >"${pid_dir}/test_server.pid"
wait_ready "${server_pid}" http://127.0.0.1:30001/health

"${workspace}/target/debug/edgion-controller" -c "${EDGION_WORK_DIR}/controller.yaml" \
  --work-dir "${EDGION_WORK_DIR}" --log-dir "${log_dir}" \
  --admin-listen 127.0.0.1:12101 --grpc-listen 127.0.0.1:12151 \
  --probe-listen 127.0.0.1:12100 --metrics-listen 127.0.0.1:12190 \
  >"${log_dir}/controller.log" 2>&1 &
controller_pid=$!
owned_pids+=("${controller_pid}")
printf '%s\n' "${controller_pid}" >"${pid_dir}/controller.pid"
wait_ready "${controller_pid}" http://127.0.0.1:12100/ready

"${workspace}/target/debug/edgion-gateway" -c "${EDGION_WORK_DIR}/gateway.yaml" \
  --work-dir "${EDGION_WORK_DIR}" --server-addr https://127.0.0.1:12151 \
  --integration-testing-mode --admin-listen 127.0.0.1:12001 \
  --probe-listen 127.0.0.1:12000 >"${log_dir}/gateway.log" 2>&1 &
gateway_pid=$!
owned_pids+=("${gateway_pid}")
printf '%s\n' "${gateway_pid}" >"${pid_dir}/gateway.pid"
wait_ready "${gateway_pid}" http://127.0.0.1:12000/ready

# Keep the client owned too, so timeout/cancellation terminates it before peers.
bash "${canonical_script}" &
client_pid=$!
owned_pids+=("${client_pid}")
wait "${client_pid}"
