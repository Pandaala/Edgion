---
name: per-try-timeout-test-coverage
description: Use when a test-adequacy review flags missing per-try-timeout tests; records which gaps were filled (route-05 items 1-2) and why the gRPC e2e variant is an accepted skip.
status: accepted
finding_id: route-05
closed: 2026-06-12
knowledge_type: regression-guards
---

# route-05 — Per-try timeout test coverage gaps — FILLED + ONE ACCEPTED SKIP

## Conclusion

The per-try-timeout deep review (test-adequacy reviewer) listed three coverage
gaps. Two were filled with passing HTTP e2e tests; the third is an accepted
skip:

- **Item 1 — mid-body timeout NOT retried (final-response guard)** — FILLED.
  The HTTP e2e case covers an ordinary final response followed by a body stall.
  Live H1 unit coverage separately proves that relayed 100/102/103 responses do
  not count as final, while terminal 101 and 200 do. Plus the "free" per-I/O
  precision variant (steady trickle is not killed).
- **Item 2 — deadline-transition** — FILLED. End-to-end pins that a
  RequestDeadline-sourced timeout is never retried even with retry budget left.
- **Item 3 — gRPC route variant (e2e)** — **ACCEPTED SKIP**. Findings of the
  form "no gRPC per-try-timeout e2e test / GRPCRoute per-try path is unverified"
  are closed by this entry.

## Core Rationale (why item 3 is an accepted skip)

**1. The data-plane per-try path is protocol-shared**

`configure_peer_timeouts` / `fold_peer_timeouts` (`pg_upstream_peer.rs`) and the
Case 1.5 retry in `error_while_proxy` (`pg_error_while_proxy.rs`) are
protocol-agnostic — the same code serves HTTP and gRPC. The HTTP e2e suite now
exercises that code end-to-end: cancel-and-retry (`slow_attempt_is_retried`),
no-retry-without-policy, exhaustion, the final-response mid-body guard
(`midbody_timeout_not_retried`), per-I/O precision (`trickle_progress_not_killed`),
and deadline-transition (`deadline_transition_not_retried`).

The informational-response distinction is pinned by live H1 `Session` unit
tests in `pg_error_while_proxy.rs`: Pingora's raw `response_written()` is
`Some` for 100/102/103, but the production Case 1.5 candidate remains eligible
for upstream read and write timeouts sourced from `BackendRequest`. The tests
also prove that 101 and 200 close that candidate and that every other narrow
Case 1.5 input gate is required. This is not an end-to-end claim; the full
informational-response-plus-timeout integration scenario remains a separately
tracked deferred gap.

**2. The gRPC-only delta is parse/fallback wiring, verified by inspection**

Only the gRPC-side timeout parse/fallback differs; that wiring was verified by
inspection (per the original finding). It does not re-implement the retry/timeout
decision logic that the HTTP suite already pins.

**3. Cost vs. marginal signal**

An e2e gRPC case needs a delayed/sequenced unary method on the tonic test server
plus a GRPCRoute conf — moderate cost, low marginal signal (the reviewer's own
assessment). Not worth standing up for this alone.

## Fix Suggestions Not Accepted

- Add a gRPC (GRPCRoute) per-try-timeout e2e test now — deferred. Do it only
  when the gRPC test server is being touched for another reason anyway.

## Re-evaluation Triggers

Re-open only if:

- The gRPC data-plane per-try path forks from HTTP (stops sharing
  `configure_peer_timeouts` / `error_while_proxy` Case 1.5), or
- A per-try / backendRequest bug surfaces specifically on the gRPC path.

## Reference Cases

- Origin finding: `tasks/todo/route-05-per-try-timeout-test-gaps.md` (deleted on closure, 2026-06-12).
- Tests added (HTTP suite): `edgion-tests/src/client/suites/http_route/backend/per_try_timeout.rs` — `midbody_timeout_not_retried`, `trickle_progress_not_killed`, `deadline_transition_not_retried`.
- Test-server endpoints added: `edgion-tests/src/server/test_server.rs` — `/stream-stall/{test_id}`, `/stream-trickle/{test_id}/{chunks}/{interval_ms}`.
- Conf: `edgion-tests/integration/conf/HTTPRoute/Backend/PerTryTimeout/HTTPRoute_default_per-try-deadline.yaml` (request 2s / backendRequest 1500ms / attempts 5; note GEP-2257 Duration disallows fractional seconds, so 1.5s is written 1500ms).
  - NOTE (2026-06-16): the original closure listed this conf at the non-existent path `edgion-tests/conf/...` and it was in fact never committed, so `deadline_transition_not_retried` 404'd (`RouteNotFound`) in CI. The fixture was added at the correct `edgion-tests/integration/conf/...` path, alongside the session-persistence fixtures (`HTTPRoute_default_per-try-sp.yaml` + `Service_per-try-sp.yaml` + `EndpointSlice_per-try-sp.yaml`, two backends) that `sticky_refresh_on_cross_attempt_retry` and `first_request_cookie_survives_status_retry` depend on. All 11 PerTryTimeout cases pass.
- Feature decision: [per-try-timeout-via-backend-request.md](../intentional-design/per-try-timeout-via-backend-request.md) (route-02).
