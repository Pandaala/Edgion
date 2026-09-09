#!/usr/bin/env bash
# Replace/Wrap are Gateway-startup-global settings, so the shared integration
# Gateway cannot exercise both. Run isolated modes serially with owned processes,
# ports, and work directories. Existing integration suites remain unchanged.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../utils/shell_compat.sh"
exec python3 "${SCRIPT_DIR}/../utils/run_with_deadline.py" \
    --timeout-secs "${EDGION_DEBUG_WIRE_TIMEOUT_SECONDS:-300}" -- \
    python3 "${SCRIPT_DIR}/test_debug_access_log_wire.py" "$@"
