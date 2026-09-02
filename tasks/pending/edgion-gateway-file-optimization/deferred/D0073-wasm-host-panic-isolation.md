# D0073 — Isolate Wasm Host Panics at VM Boundaries

## Meta
- Status: needs-implementation
- Severity: high
- Discovered in: B0109 analysis; resolved by independent B0108/B0109 adjudication
- Category: panic-isolation
- Canonical task: [`tasks/todo/isolate-wasm-host-panics-at-vm-boundaries.md`](../../../todo/isolate-wasm-host-panics-at-vm-boundaries.md)

## Evidence
Wasmtime 46 catches Rust host-function panics only to carry them safely across Wasm/JIT frames, then resumes unwinding at the typed-call boundary. Edgion currently handles returned traps/errors and poisons the lease afterward, but a resumed host panic bypasses that path. Root, request, callout-response, tick, close, and related typed-call boundaries lack a project-owned redacted panic scope, so a panic can escape toward the Pingora worker and may return a VM to the pool without poisoning.

## Why deferred
The correction belongs to the future `edgion-gateway/src/plugins/http/wasm/vm_manager.rs` coverage batch and its inline tests. Catching only in the facade, plugin, or individual host imports would leave other typed-call paths exposed. B0108's independent host-memory hardening and B0109's facade no-change decision should not expand.

## Integration-test impact
Existing panic recovery coverage exercises a guest trap, not a Rust host-import panic. Add a deterministic crate-local panicking-host test that proves generic error conversion, poison/discard, and recovery. No integration-test edit or separate integration issue is needed.
