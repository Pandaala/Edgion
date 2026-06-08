#!/usr/bin/env bash
# =============================================================================
# cli-get-target-incompat.sh
#
# Verifies `edgion-cli --target server get <kind> --metadata-only` exits
# non-zero with the specific stderr guidance (Card 05).
#
# Does not require a running controller: the compat check fires before any
# network call (pre-flight check in cli/mod.rs, before ensure_auth).
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
CLI="${PROJECT_ROOT}/target/debug/edgion-cli"

[[ -x "$CLI" ]] || { echo "SKIP: edgion-cli not built"; exit 0; }

set +e
STDERR=$("$CLI" --target server --server "http://127.0.0.1:12101" get httproute --metadata-only 2>&1 >/dev/null)
EXIT_CODE=$?
set -e

[[ "$EXIT_CODE" -ne 0 ]] \
    || { echo "FAIL: expected non-zero exit, got 0"; echo "$STDERR"; exit 1; }

echo "$STDERR" | grep -q -- "--metadata-only only works with --target center" \
    || { echo "FAIL: stderr missing expected guidance"; echo "stderr was: $STDERR"; exit 1; }

echo "PASS: cli-get-target-incompat.sh"
