#!/usr/bin/env bash
# Enforce that edgion-resources is a true leaf crate: its sources must not
# reference crate::core:: . Run directly (CI / pre-commit). Exits non-zero (and
# prints the offending lines) on violation.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
TARGET_DIR="${REPO_ROOT}/edgion-resources/src"

if grep -rqE "crate::core::" "$TARGET_DIR" --include='*.rs'; then
    echo "ERROR: edgion-resources/ must not reference crate::core:: (it is a leaf crate)" >&2
    grep -rnE "crate::core::" "$TARGET_DIR" --include='*.rs' >&2
    exit 1
fi
echo "OK: edgion-resources/ is a clean leaf (no crate::core:: edges)"
