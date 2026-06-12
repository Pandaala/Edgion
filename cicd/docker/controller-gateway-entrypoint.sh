#!/bin/sh
# Controller-Gateway entrypoint: starts edgion-controller + edgion-gateway in a
# single container.

set -e

cleanup() {
    echo "[controller-gateway] shutting down..."
    kill $(jobs -p) 2>/dev/null || true
    wait
    exit 0
}
trap cleanup SIGTERM SIGINT

echo "[controller-gateway] starting edgion-controller (admin 0.0.0.0:12101, probe 0.0.0.0:12100)..."
/usr/local/bin/edgion-controller &

echo "[controller-gateway] starting edgion-gateway..."
/usr/local/bin/edgion-gateway &

# Wait for any child to exit; propagate its exit code
wait -n
exit_code=$?
echo "[controller-gateway] a process exited (code ${exit_code}), shutting down..."
kill $(jobs -p) 2>/dev/null || true
wait
exit ${exit_code}
