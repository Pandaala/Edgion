#!/usr/bin/env bash
# Manually run the CRD <-> struct field-coverage guard.
#
# This wraps the Rust integration test `edgion-resources/tests/crd_coverage.rs`,
# which fails when a struct field on EdgionGatewayConfig / EdgionTls is missing
# from (Direction-1) or stale in (Direction-2) its hand-written CRD YAML, and
# also pins the acme `status.activeChallenges` and stream `tlsRoutePlugins`
# fixes. The same test runs in CI via `cargo test --all`; this script is the
# convenient local entry point.
#
# Exit 0 when the CRDs are in sync; exit 1 with the drift report otherwise.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

exec cargo test -p edgion-resources --test crd_coverage -- --nocapture
