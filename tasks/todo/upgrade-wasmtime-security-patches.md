# Upgrade Wasmtime for RUSTSEC-2026-0268 and RUSTSEC-2026-0269

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | completed |
| Type | security |
| Priority | P1 |
| Issue | N/A |
| Discovered by | B0093 verification |

## AI guide summary

Upgrade the Wasmtime 46 family to the patched 46.0.3 release without enabling WASI or expanding the Edgion Proxy-Wasm host boundary, then restore clean supply-chain gates.

## Requirements

- Resolve Wasmtime to at least 46.0.3 and below 47.0.0, keeping the compatible Pulley and Cranelift family coherent.
- Inspect and record the exact lockfile package, version, source, checksum, dependency-edge, and feature-graph changes before implementation approval.
- Preserve the runtime's exact Edgion Proxy-Wasm host ABI and the analyzer's rejection of Components, WASI imports, and unknown host modules.
- Do not add `wasmtime-wasi`, filesystem preopens, ambient networking, or another host capability.
- Do not add an ignore for RUSTSEC-2026-0268 or RUSTSEC-2026-0269.
- Preserve the independently approved Pingora 2e2 lock intent needed by the current Gateway source.
- Do not modify existing integration tests.

## Acceptance criteria

- `cargo tree --locked` shows a coherent patched Wasmtime/Pulley/Cranelift family and no WASI runtime dependency or new host capability.
- Edgion Wasm analysis, module compilation, instantiation, host ABI, tick/callout, limit, and failure-isolation tests pass.
- Complete Gateway library tests, locked workspace check/Clippy, and the Gateway binary build pass.
- `cargo audit` reports zero vulnerabilities, and `cargo deny check` passes advisories without new ignores.
- Repository guards and frozen-path verification pass.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Scope and review the patched runtime upgrade independently, then incorporate it atomically into the B0093 closeout. | The advisories predate B0093, but one lockfile also owns the current-HEAD Pingora compile recovery; this does not expand the OIDC source boundary or count as Wasm/Pingora anchor coverage. |
| 2026-09-01 | Resolve the prerequisite with Wasmtime/Pulley 46.0.3 and Cranelift 0.133.3. | Exact lock, feature-graph, runtime, and supply-chain reviews passed; no WASI capability or advisory ignore was added. |

## Completion evidence

- `cargo tree --locked` resolves `wasmtime v46.0.3` for `edgion-gateway` and contains no `wasmtime-wasi`, `wasi-common`, `cap-std`, or `wiggle` runtime dependency.
- The complete Gateway library passed with 3,549 tests and 2 ignored; focused Wasm, OpenID Connect, PluginRuntime, and request-body seams also passed.
- Locked workspace check and Clippy, the locked Gateway binary build, all repository guards, and the frozen integration-path check passed.
- `cargo audit` reports zero vulnerabilities; `cargo deny check` passes without a new advisory ignore.
