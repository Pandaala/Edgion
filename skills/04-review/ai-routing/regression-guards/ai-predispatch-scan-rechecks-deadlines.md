---
knowledge_type: regression-guards
---

# AI Predispatch Scans Re-check End-to-End Deadlines

## Decision

Every AI predispatch scan iteration enters through `prepare_first_attempt`, which must check the HTTPRoute request timeout before the gRPC client deadline and before taking or mutating the attempt plan. The loop must check the same typed gate again after each I/O-bearing admission or endpoint-resolution step, before it returns a peer, emits a terminal 503, or advances to another candidate.

An expired request timeout returns `AiDispatchError::RequestDeadlineExceeded`; an expired gRPC client deadline returns `AiDispatchError::GrpcDeadlineExceeded`. The HTTP proxy maps both typed results to HTTP 504 and records the corresponding canonical `EdgionErrorCode` (`RequestTimeoutExceeded` or `GrpcClientDeadlineExceeded`). They must not fall through to the OpenAI-compatible 503 or generic 500 predispatch paths.

## Why the gate belongs in dispatch preparation

The AI peer-selection loop may scan several credentials and services within one Pingora `upstream_peer` callback. Admission and endpoint resolution can block between iterations, so a deadline check only before entering that callback becomes stale. Keeping the gate at the common dispatch-preparation entry makes credential-scoped and service-scoped skips re-check the deadline before the next candidate without relying on every loop branch to duplicate the check.

The entry gate precedes `ctx.ai_attempt_plan.take()`. Expiry there therefore preserves the ready plan, leaves the scheduler dispatch count unchanged, and does not consume retry-budget accounting or install a reservation. If a deadline expires while I/O is in flight, the post-I/O gate releases and drains any installed predispatch reservation before returning 504. It never marks a dispatch or charges retry-budget accounting.

## Regression coverage

- Live-session dispatch tests use already-expired HTTPRoute and gRPC deadlines and require the typed errors while the ready scheduler remains at `PickService` with zero dispatches.
- Proxy-layer mapping tests require the two typed errors to select the standard deadline statuses and reject unrelated predispatch errors.
- A post-I/O gate test requires an expired deadline to produce the same canonical 504/status path used by the loop.

This is an internal enforcement correction. It changes no resource schema or operator-facing configuration, so no user documentation update is required.

## Resolution references

- `gw-09` — 2026-08-09.
