#!/usr/bin/env bash
# Manually run the CRD <-> struct field-coverage guards.
#
# This wraps:
#   * `edgion-resources/tests/crd_coverage.rs` — fixed spec/status fields in
#     every served version of all nine custom CRDs. Explicit open config and
#     preserved-reference boundaries are documented in the scope inventory.
#     Detects missing/stale fields, retains the original narrow regression pins,
#     and tests the comparator with in-memory CRD mutations. Property presence
#     does not establish types, defaults, bounds or live admission equivalence.
#   * `edgion-resources/tests/plugin_crd_condition_coverage.rs` — flattened
#     `&conditionsSchema` / KeyGet type-enum guard on the EdgionPlugins CRD.
#
# The same tests run in CI via `cargo test --all`; this script is the
# convenient local entry point.
#
# Exit 0 when the CRDs are in sync; exit 1 with the drift report otherwise.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

cargo test -p edgion-resources --test crd_coverage -- --nocapture
cargo test -p edgion-resources --test plugin_crd_condition_coverage -- --nocapture
