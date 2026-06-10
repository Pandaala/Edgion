#!/bin/sh
# All-in-One entrypoint: starts backend process(es) + nginx
# ALL_IN_ONE_MODE is set at build time: "controller-gateway"
# (the "center" mode was retired; edgion-center embeds the dashboard itself)

set -e

ALL_IN_ONE_MODE="${ALL_IN_ONE_MODE:-controller-gateway}"

cleanup() {
    echo "[all-in-one] shutting down..."
    kill $(jobs -p) 2>/dev/null || true
    wait
    exit 0
}
trap cleanup SIGTERM SIGINT

case "${ALL_IN_ONE_MODE}" in
    controller-gateway)
        echo "[all-in-one] starting edgion-controller (admin 0.0.0.0:12101, probe 0.0.0.0:12100)..."
        /usr/local/bin/edgion-controller &

        echo "[all-in-one] starting edgion-gateway..."
        /usr/local/bin/edgion-gateway &

        echo "[all-in-one] starting nginx (dashboard on :5800)..."
        nginx -g 'daemon off;' &
        ;;
    *)
        echo "[all-in-one] unknown ALL_IN_ONE_MODE: ${ALL_IN_ONE_MODE}"
        exit 1
        ;;
esac

# Wait for any child to exit; propagate its exit code
wait -n
exit_code=$?
echo "[all-in-one] a process exited (code ${exit_code}), shutting down..."
kill $(jobs -p) 2>/dev/null || true
wait
exit ${exit_code}
