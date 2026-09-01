#!/usr/bin/env bash

set -euo pipefail

role="${1:-}"

case "$role" in
  cert-bootstrap)
    set +e
    EDGION_CONF_SYNC_CERTS_DIR=/certs/grpc \
      bash /workspace/edgion-tests/integration/scripts/gen_certs/generate_conf_sync_certs.sh
    bootstrap_rc=$?
    set -e
    if [ "$bootstrap_rc" -eq 0 ]; then
      for cert_file in ca.crt server.crt server.key client.crt client.key; do
        if [ ! -s "/certs/grpc/primary/${cert_file}" ]; then
          echo "ERROR: generated certificate is missing: ${cert_file}" >&2
          bootstrap_rc=1
        fi
      done
    fi
    if [ "$bootstrap_rc" -eq 0 ]; then
      echo '@@ISOLATED_BOOTSTRAP_RESULT PASS 0'
    else
      printf '@@ISOLATED_BOOTSTRAP_RESULT FAIL %s\n' "$bootstrap_rc"
    fi
    exit "$bootstrap_rc"
    ;;
  controller)
    mkdir -p /work/config /work/logs
    cp -R /workspace/config/crd /work/config/crd
    exec /usr/local/bin/edgion-controller \
      -c /scenario/config/controller.yaml \
      --work-dir /work
    ;;
  gateway)
    mkdir -p /work/config /work/logs
    cp -R /workspace/config/crd /work/config/crd
    exec env \
      RUST_LOG=info,pingora_proxy=error,pingora_core=error \
      EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
      /usr/local/bin/edgion-gateway \
        -c /scenario/config/gateway.yaml \
        --work-dir /work \
        --server-addr https://edgion-controller:12151 \
        --integration-testing-mode \
        --admin-listen 0.0.0.0:12160 \
        --probe-listen 0.0.0.0:12161
    ;;
  test-server)
    test_server_port="${TEST_SERVER_PORT:?TEST_SERVER_PORT is required}"
    exec /usr/local/bin/test_server --http-ports "$test_server_port" --log-level info
    ;;
  wire-dump)
    exec python3 /usr/local/lib/edgion/proxy_wire_dump.py
    ;;
  *)
    echo "ERROR: unknown proxy topology component role: ${role}" >&2
    exit 2
    ;;
esac
