---
name: wasm-body-counter-delta-rerun-safe
description: Use when reviewing Wasm request-body integration counters; covers rerun-safe assertions over shared module state.
status: fixed
finding_id: BODY-010
closed: 2026-07-27
knowledge_type: regression-guards
---

# Wasm Body Counter Assertions Use Per-Request Deltas — FIXED

## Conclusion

Integration tests that observe Wasm shared counters must compare values before
and after the request under test. Absolute counter assertions are not accepted
because module state can survive multiple requests and repeated suite runs.

## Core Rationale

**1. Shared state is intentionally cumulative**

The reference Wasm guest increments request-body byte, callback, and terminal
callback counters in shared module state. The counters describe the module
lifetime, not one test-client process.

**2. A delta proves the per-request contract**

Reading `(bytes, calls, eos)` immediately before and after the body-producing
request isolates that request's contribution. The expected delta `(11, 1, 1)`
proves that the complete `hello world` body arrived in one terminal callback
without assuming a fresh gateway or zeroed module state. Checked subtraction
turns any unexpected counter reset into a diagnostic test failure instead of an
unsigned-underflow panic.

**3. Reload verification protects the real failure mode**

The focused `BodyPlugins` suite passes twice against one unchanged Gateway PID
and also passes both before and after a Controller and Gateway reload. Together
these checks verify that prior requests and reconstructed runtime state cannot
produce a false failure.

## Fix Suggestions Not Accepted

- Reset shared counters before every test — that adds a test-only mutation path
  and can hide state-lifetime defects.
- Require a fresh gateway process — repeated and reload-mode runs are supported
  integration workflows.
- Assert only the byte delta — callback count and terminal state are part of
  the whole-body delivery contract.

## Re-evaluation Triggers

Re-open this decision only if:

- The reference guest replaces cumulative counters with request-scoped state.
- A canonical integration isolation mechanism guarantees a new Wasm module
  instance for every test case.
- Request-body delivery changes from one complete terminal callback.

## Reference Cases

- `BODY-010`
- `edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs`
- `edgion-tests/wasm/header-demo/src/lib.rs`

## Resolution references

- `BODY-010` — 2026-07-27.
