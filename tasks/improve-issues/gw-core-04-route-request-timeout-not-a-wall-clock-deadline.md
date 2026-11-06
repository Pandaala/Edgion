# gw-core-04: `timeouts.request` is not a wall-clock deadline once the upstream exchange starts

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs:1037-1122` (`configure_peer_timeouts`: `request_remaining` becomes `peer.options.read_timeout` / `write_timeout`, lines 1101-1103)
  - `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs:76-90` (deadline checked only before peer selection)
  - Pinned Pingora `pingora-core/src/protocols/http/v1/client.rs:~479-486` (`read_body_ref` wraps each read in `timeout(read_timeout, ...)`)
  - `docs/en/user-guide/http-route/resilience/timeouts.md:36-47`

## Summary
The route `timeouts.request` (GEP-1742) is checked only at peer selection and retry decision points. After
that, its remaining budget is copied into the upstream connection's per-operation `read_timeout` and
`write_timeout`. Pingora re-arms those timers on every read and write, so any upstream that keeps producing
a byte more often than the remaining budget never hits the deadline. A slow or drip-feeding backend can hold
a request, and its downstream connection and memory, for an unbounded time even though the operator set an
"end-to-end budget for the whole transaction". The user docs promise exactly that, then partly retract it
in a "precision" note.

## Current behavior (evidence)
- `upstream_peer` returns 504 only if `elapsed >= request_timeout` at selection time (`pg_upstream_peer.rs:76-90`).
- `configure_peer_timeouts` computes `request_remaining = request_timeout - elapsed` and folds it with
  `backendRequest` and `grpc-timeout` into a single duration, then sets
  `peer.options.read_timeout = Some(timeout); peer.options.write_timeout = Some(timeout);`
  (`pg_upstream_peer.rs:1098-1103`).
- Pingora's H1 client applies `read_timeout` per call (`timeout(t, self.do_read_body())` in
  `read_body_ref`), and the H2 path does the same per frame or read. The budget is never decremented across
  reads.
- The only total-deadline machinery in the fork is `wait_with_response_head_deadline`, which applies only to
  the response *head* hold (used by the bounded AI and gRPC-JSON converters). Ordinary body relay has no
  wall-clock bound.
- `timeouts.md:36` says "`request`: The end-to-end budget for the whole transaction ... When the budget is
  exhausted the request is terminated". `timeouts.md:43-45` then says budgets are enforced "as per-operation
  read/write timeouts, so an attempt that keeps making progress is not killed mid-stream".

## Industry comparison
- **Gateway API GEP-1742**: `request` is "the maximum duration for a gateway to respond to an HTTP request.
  If the gateway has not been able to respond before this deadline is met, the gateway MUST return a
  timeout error". The GEP's timeline diagram covers the full response.
- **Envoy** (`RouteAction.timeout`, which Envoy Gateway and Istio map `timeouts.request` to): a wall-clock
  timer runs from downstream end-of-stream until the *upstream response is complete*. A drip-fed response
  is reset when it fires. Idle behavior is a separate knob (`idle_timeout`, `per_try_idle_timeout`).
  Streaming use cases disable `timeout` explicitly.
- **NGINX**: `proxy_read_timeout` is per-read (idle) by design, but NGINX does not present it as a total
  request budget.

## Impact
- Semantics: `timeouts.request` behaves like an idle timeout after the first byte, which contradicts the spec
  MUST and Edgion's own docs. Operators who set it as a resource bound, for example to cap slow LLM calls or
  expensive reports, do not get one.
- Resource exhaustion: a backend that trickles bytes, such as a misbehaving service, a compromised upstream
  or a slow-loris-style dependency, pins the downstream connection, response-processing state and any
  bounded windows indefinitely. Retries, budgets and outlier logic never see a failure.
- Inconsistency: the same deadline *is* treated as absolute for connect (`fold_connect_timeouts`), retry
  backoff, and response-head holds, but not for the body.

## Suggested direction
- Keep the per-I/O folding, which is still useful as the idle/per-try bound. In addition, arm one absolute
  deadline for `timeouts.request` (and the client `grpc-timeout`) that covers the whole proxy lifecycle:
  `start_time + request` for the request, with the gRPC deadline handled the same way. Two places can arm it:
  - The Pingora fork, which already carries a deadline primitive (`wait_with_response_head_deadline`).
    Generalise it to a whole-exchange `request_deadline` checked in the body relay loops (`proxy_h1`,
    `proxy_h2`) with a `sleep_until` branch in the existing `select!`.
  - Or, without a fork change, a per-request `tokio::time::sleep_until` registered in
    `upstream_response_body_filter_event` that returns an error once elapsed. This is coarser because it
    only fires when a chunk arrives.
- On expiry, reuse the existing outcome mapping. Before header commit this is 504 or `DEADLINE_EXCEEDED`.
  After commit, abort the stream: H2 `RST_STREAM`, H1 connection close. Record `RequestTimeoutExceeded`.
- Rewrite `timeouts.md` so that `request` is a hard total deadline (with `"0s"` to disable for streaming)
  and `backendRequest` is per-try plus idle, matching Envoy's `timeout` versus `per_try_idle_timeout` split.

## Notes
- Checked `skills/04-review/routing/intentional-design/per-try-timeout-via-backend-request.md`. It accepts
  per-I/O semantics for **`backendRequest`** as the per-try bound, which is reasonable and not challenged
  here. It says nothing about `request`.
- The user-doc "precision" note is the only place this behavior is acknowledged for `request`. It
  contradicts the same page's definition and GEP-1742's MUST, and no intentional-design page accepts it.
  That is the new evidence for filing.
- Related guards that already treat the request deadline as absolute in other phases:
  `routing/regression-guards/connect-failure-after-deadline-uses-timeout-outcome.md` and
  `routing/regression-guards/retry-backoff-minimum-wait-conformance.md`. No `tasks/` item tracks this.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs:76-90`: the absolute check runs only before peer selection.
- `pg_upstream_peer.rs:1097-1103`: the remaining budget computed once at selection is copied into `peer.options.read_timeout`/`write_timeout`, which Pingora re-arms per I/O; no `sleep_until`/whole-exchange deadline exists in `proxy_http/` (only `pg_fail_to_connect.rs` consults `request_deadline_exceeded`).
- `docs/en/user-guide/http-route/resilience/timeouts.md:36` promises a whole-transaction budget, then `:43-45` retracts it as per-operation; `skills/04-review/routing/intentional-design/per-try-timeout-via-backend-request.md` accepts per-I/O semantics only for `backendRequest`, not `request`.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs:1102-1103`: `peer.options.read_timeout`/`write_timeout = Some(timeout)` unchanged (no diff to `proxy_http/`).
- `docs/en/user-guide/http-route/resilience/timeouts.md` has no diff; Pingora fork commit unchanged.
