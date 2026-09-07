# D0073 — Isolate Wasm Host Panics at VM Boundaries

## Meta
- Status: resolved
- Severity: high
- Discovered in: B0109 analysis; resolved by independent B0108/B0109 adjudication
- Category: panic-isolation
- Canonical task: [`tasks/todo/isolate-wasm-host-panics-at-vm-boundaries.md`](../../../todo/isolate-wasm-host-panics-at-vm-boundaries.md)

## Evidence
Wasmtime 46 catches Rust host-function panics only to carry them safely across Wasm/JIT frames, then resumes unwinding at the typed-call boundary. Edgion currently handles returned traps/errors and poisons the lease afterward, but a resumed host panic bypasses that path. Root, request, callout-response, tick, close, and related typed-call boundaries lack a project-owned redacted panic scope, so a panic can escape toward the Pingora worker and may return a VM to the pool without poisoning.

## Resolution
B0115 wrapped every Wasmtime initialization and runtime guest boundary with the canonical redacted panic catcher. Initialization panics fail before publication; runtime panics retain the existing phase metric and poison/discard the VM. Both tick failure paths stop before reading guest state, and deterministic inline host-panic tests prove redaction, discard, ordinary-trap distinction, and recovery.

## Integration-test impact
Existing panic recovery coverage exercises a guest trap, not a Rust host-import panic. Add a deterministic crate-local panicking-host test that proves generic error conversion, poison/discard, and recovery. No integration-test edit or separate integration issue is needed.
