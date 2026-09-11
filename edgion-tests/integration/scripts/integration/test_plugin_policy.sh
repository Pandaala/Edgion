#!/usr/bin/env bash
# Run the EGC policy lifecycle against the already-running shared topology.
# The canonical full runner discovers PluginPolicyDynamic through test_client.
# This focused entry point never starts or restarts Controller/Gateway processes.
# After final cleanup, prepare a fresh shared topology before running it again;
# see tasks/todo/http-listener-recreation-stale-route-manager.md.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
TEST_CLIENT="${EDGION_TEST_CLIENT_BIN:-${PROJECT_ROOT}/target/debug/test_client}"
if [[ -z "${EDGION_WORK_DIR:-}" ]]; then
    EDGION_WORK_DIR="$(cat "${PROJECT_ROOT}/tmp/integration_testing/.current")"
    export EDGION_WORK_DIR
fi
if [[ ! -x "${TEST_CLIENT}" ]]; then
    echo "test_client binary is missing: ${TEST_CLIENT}" >&2
    exit 1
fi
cd "${PROJECT_ROOT}"
exec "${TEST_CLIENT}" -g --dynamic-scenario PluginPolicyDynamic "$@"
