#!/usr/bin/env bash
# Manually run the CRD <-> struct field-coverage guards.
#
# This wraps:
#   * `edgion-resources/tests/crd_coverage.rs` — structural CRDs
#     (EdgionGatewayConfig / EdgionTls / EdgionBackendTrafficPolicy /
#     EdgionBackend). Fails when a struct field is missing from (Direction-1)
#     or stale in (Direction-2) its hand-written CRD YAML, and also pins the
#     acme `status.activeChallenges` and stream `tlsRoutePlugins` fixes.
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
