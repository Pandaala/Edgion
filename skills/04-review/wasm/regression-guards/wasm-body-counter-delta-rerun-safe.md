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

## Background tick observation windows

The separate `Wasm` tick/shared-data and tick-callout cases take a successful
baseline and require a positive checked delta from later probes (ARCH-401).
The guest's counters belong to the request-stage runtime lifetime; an unchanged
positive value can outlive the tick or a healthy callout dependency. Both probes
retain their own marker/header and the advanced fixture host. Missing, malformed
or reset counters fail, as do HTTP/JSON/request errors. A ten-second deadline
covers the baseline and all subsequent probes, with 200 ms between observations.
An unchanged value at the deadline fails; the harness never resets shared state.

This proves background progress during the observation window, not attribution
to a particular request or sustained service health. The pure `tick_tests` cover
numeric parsing and checked deltas, including nonzero baselines and resets; they
do not establish a fresh live Gateway/Kubernetes tick or callout run. Runtime
ownership, tick cancellation, egress permissions and guest code are unchanged.
