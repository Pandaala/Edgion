# D0062 — Upgrade Wasmtime for RUSTSEC-2026-0268 and RUSTSEC-2026-0269

## Meta
- Status: resolved
- Severity: high
- Discovered in: B0093, dependency verification
- Category: security-supply-chain
- Canonical task: [`tasks/todo/upgrade-wasmtime-security-patches.md`](../../../todo/upgrade-wasmtime-security-patches.md)

## Evidence
Both the unchanged `142e0de0` baseline lock and the initial B0093 lock resolved Wasmtime 46.0.2. Current RustSec data reports RUSTSEC-2026-0268, a guest-controlled host allocation in WASIp3 streams, and RUSTSEC-2026-0269, a high-severity filesystem sandbox escape involving trailing slashes or symlinks. `cargo deny check` and `cargo audit` failed on both advisories.

The current Gateway does not link `wasmtime-wasi`, `cap-std`, or `wasi-common`; its analyzer rejects Components and non-Edgion host import modules. That reduces the demonstrated reachable surface but is not a basis for ignoring a direct runtime advisory when a patched 46.0.3 release is available.

## Resolution
The lockfile now resolves Wasmtime and Pulley 46.0.3 and Cranelift 0.133.3 while preserving the independently required Pingora `2e2e6741` revision. The exact lock diff received separate plan and final reviews; it added no WASI runtime, filesystem preopen, ambient-networking, or host-ABI capability.

Because one lockfile owns both the current-HEAD Pingora compile recovery and this security correction, the separately scoped prerequisite is incorporated atomically into the B0093 closeout. It does not expand the OIDC source boundary or count as Wasm/Pingora anchor coverage.

The complete Gateway library, focused analyzer/runtime/host-ABI tests, locked workspace check and Clippy, the Gateway binary build, repository guards, and frozen-path check passed. `cargo audit` now reports zero vulnerabilities and `cargo deny check` passes without a new advisory ignore.

## Integration-test impact
No new integration issue is required. Existing Proxy-Wasm/Wasm focused tests plus the full Gateway library and current integration scenarios are sufficient unless the exact upgrade diff introduces a new runtime boundary.
