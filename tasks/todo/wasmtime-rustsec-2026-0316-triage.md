# Triage Wasmtime RUSTSEC-2026-0316

## Status and scope

Pending dependency/security triage, outside the WAF tokenizer batch. Do not
infer an exploitable Gateway path from the lockfile advisory alone.

## Evidence

On 2026-09-29, cargo-audit 0.22.2 against RustSec database revision
`f23b768236fe2880e4cfa167da662cad8ca79240` reported Wasmtime 46.0.3 under
`RUSTSEC-2026-0316` / `GHSA-jqpg-j7w6-42pr`. Both baseline `582dae37`
and the WAF S2 working lockfile produce this same finding. No Wasmtime package
version or feature was changed by S2. `deny.toml` has no entry for this advisory.

The [upstream advisory](https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-jqpg-j7w6-42pr)
concerns host allocations while lifting dynamically typed component values.
The published patched branches are 36.0.16, 48.0.3 and 49.0.1; these are
historical inspection data, not an instruction to choose a version blindly.

Relevant local sources:

- `Cargo.toml`: workspace Wasmtime version requirement is `46`.
- `Cargo.lock`: resolved Wasmtime 46.0.3.
- `edgion-gateway/src/plugins/http/wasm/vm_manager.rs`: core module VM construction.
- `edgion-gateway/src/plugins/http/wasm/host_abi.rs`: Proxy-Wasm core host ABI.
- `edgion-wasm/src/lib.rs`: component encoding is rejected as `ComponentUnsupported`.

The visible production path uses core modules and rejects components, which
suggests the affected component API may be unreachable. This is an inference,
not a completed security review; verify every loader/VM path before closing.

## Work and acceptance

1. Read the upstream advisory and current Wasmtime feature/API use. Trace every
   supported artifact loader and any dynamically typed component value path.
2. If unreachable, document the precise boundary with an appropriate regression
   test and a justified advisory-policy entry only after independent review.
3. If reachable, select a supported patched Wasmtime branch, update the complete
   dependency family, and verify host ABI, limits, artifact rejection, cleanup,
   and Gateway integration. Do not make an unrelated compatibility refactor.
4. Run workspace checks and dependency audit using a current pinned advisory
   database; record exact versions, reachable scope, and remaining limitations.

Do not suppress this finding or claim a production vulnerability without the
reachability evidence. WAF S2 did not modify advisory policy.
