# D0073 — Isolate Wasm Host Panics at VM Boundaries

## Meta
- Status: resolved
- Severity: high
- Discovered in: B0109 analysis; resolved by independent B0108/B0109 adjudication
- Category: panic-isolation
- Closed: 2026-09-07
- Resolution commit: `b09156e3`

## Evidence
Before B0115, Wasmtime 46 carried Rust host-function panics safely across
Wasm/JIT frames and resumed unwinding at the typed-call boundary. Edgion handled
returned traps and errors but lacked a project-owned redacted panic scope around
root, request, response, body, callout-response, tick, close, and initialization
boundaries. A resumed host panic could therefore escape toward the Pingora
worker and bypass VM poisoning.

## Resolution
B0115 wrapped every Wasmtime initialization and runtime guest boundary with the canonical redacted panic catcher. Initialization panics fail before publication; runtime panics retain the existing phase metric and poison/discard the VM. Both tick failure paths stop before reading guest state, and deterministic inline host-panic tests prove redaction, discard, ordinary-trap distinction, and recovery.

## Current verification

Revalidated on 2026-09-10. `run_guest_boundary` still maps only caught Rust
panics to the fixed generic error. Initialization, request/response headers and
bodies, callout response, tick, done, and delete calls use that boundary; runtime
errors poison the lease so it cannot return to the pool. Five focused tests
passed: initialization redaction/nonpublication, the complete runtime phase
matrix with discard and recovery, ordinary guest-trap distinction, direct tick
panic discard, and tick-callout callback panic discard.

## Integration-test impact
Crate-local panicking-host tests cover generic error conversion,
poison/discard, ordinary guest-trap distinction, and recovery. No integration
test edit or separate integration issue is needed.
