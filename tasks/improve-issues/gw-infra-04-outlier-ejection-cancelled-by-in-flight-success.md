# gw-infra-04: Passive outlier ejection is cancelled by any straggling successful response

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**: `edgion-gateway/src/backends/outlier_detection/store.rs:215-260` (`record_success_for_backend_in`), `backends/outlier_detection/state.rs:256-270` (`PassiveEjectionState::record_success`), `backends/discovery/coordinator.rs:467-475` (`EndpointRuntimeState::clear_outlier_ejected`), `routes/http/proxy_http/pg_upstream_response_filter.rs:170-178`, `routes/tcp/edgion_tcp.rs:278`, `routes/tls/proxy.rs:400`, `routes/tls/passthrough_proxy.rs:318`, `backends/mod.rs:531`

## Summary
After an endpoint is ejected, the first successful completion recorded for it lifts the ejection
immediately. Ejected endpoints are excluded from selection, so those completions come from requests that were
already in flight when the ejection happened. On any endpoint with concurrency above about 1, an intermittently
failing backend is un-ejected within milliseconds, and the ejection duration, exponential backoff and
`maxEjectionPercent` do almost nothing. Envoy and other gateways use time-based ejection: a success resets
the consecutive-failure counter but does not end an active ejection.

## Current behavior (evidence)
- The completion token is captured at selection time (`backends/mod.rs:531`,
  `capture_completion_token`), so every request selected before the ejection carries a valid token. The token
  binds only the policy generation, not the ejection generation.
- `pg_upstream_response_filter.rs:174-177`: any response status below 500 calls
  `record_success_for_backend_with_token`. TCP/TLS proxies do the same on a successful session.
- `store.rs:248-257`: inside `with_current_outlier_endpoint`, success unconditionally runs
  `endpoint.record_success(); runtime.clear_outlier_ejected();`.
- `state.rs:240-251`: `record_success` does `state.open_until.take()`, resets all failure counters and bumps
  the generation. `clear_outlier_ejected` clears the selection gate and the gauge.
- Sequence with `consecutiveGatewayErrors: 5` and 50 concurrent requests to a backend that returns 503 on half
  of its requests: five consecutive 503s open the ejection for `ejection_seconds x num_ejections`. Within the
  same RTT one of the about 45 still-in-flight requests completes with 200 and the ejection is gone. The
  backend keeps receiving full traffic, and `edgion_circuit_breaker_ejections_total` keeps increasing with no
  protective effect. The same applies to `LocalOrigin` ejections, because a successful request that connected
  just before the failures cancels them.
- `num_ejections` keeps growing, so the backoff value grows, but no ejection lasts long enough for it to matter.

## Industry comparison
- Envoy outlier detection: an ejected host stays ejected for `base_ejection_time x num_ejections` (capped by
  `max_ejection_time`), and is un-ejected only by the interval timer. A success resets the consecutive
  counters only. `num_ejections` decays by one per healthy interval. Envoy's `successful_active_health_check_uneject_host`
  lets an *active* HC success uneject; passive request successes never do.
- Istio `DestinationRule.outlierDetection` and Kong passive healthchecks (`healthy.successes` counts only
  while the target is not disabled, and re-enable requires active checks or manual action) follow the same
  model.

## Impact
Passive outlier detection, which the CRD exposes as a circuit breaker, gives little protection for partially
failing or overloaded backends: the cases it exists for. Operators see ejection metrics firing and
assume protection, while traffic is still routed to the bad endpoint.

## Suggested direction
1. While an ejection is active, a passive success must not clear `open_until` or the runtime gate. Only reset
   the consecutive counters, as Envoy does. Ejection ends only by deadline (the lazy expiry already in
   `EndpointRuntimeState::outlier_ejected`) or by an explicit active-HC success, if that is ever wired.
2. Alternatively, fence the completion token by ejection generation. Capture `PassiveEjectionState`'s
   generation at selection, and drop success and failure completions whose generation is older than the
   current ejection. This also stops pre-ejection failures from counting toward the next ejection.
3. Add Envoy-style decay of `num_ejections` during healthy periods so backoff does not grow without bound
   once (1) makes ejections effective.
4. Keep the gauge invariants from `circuit-breaker-record-success-gauge-leak.md`: the gauge still settles
   exactly once, now on deadline expiry instead of on success.

## Notes
`skills/04-review/backends/regression-guards/circuit-breaker-record-success-gauge-leak.md` documents that
current success "calls `EndpointRuntimeState::clear_outlier_ejected`". That page covers gauge balance (no
double decrement), not whether success *should* end an ejection, and it gives no rationale for that semantics.
The new evidence is the in-flight straggler path above, which makes the ejection ineffective under concurrency.
Also checked `circuit-breaker-local-origin-split.md` (counter split only) and
`circuit-breaker-integration-ejection-state-self-cleaning.md`. The AI credential outlier path
(`ai/runtime/attempt_runtime.rs`) uses the half-open `admit()`/`HalfOpenProbe` model and is not affected.
Not tracked in `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged.
- `outlier_detection/store.rs:248-257`: in `with_current_outlier_endpoint`, a success unconditionally runs `endpoint.record_success(); runtime.clear_outlier_ejected();`. The token check (`:234-236`) compares only the policy identity, not the ejection generation.
- `outlier_detection/state.rs:256-270` `record_success` does `state.open_until.take()` and resets every failure counter. Only an explicit (Retry-After) cooldown survives, and that path belongs to AI credentials, not passive Service ejection.
- `pg_upstream_response_filter.rs:170-178`: any status not classified as a failure calls `record_success_for_backend_with_token`, so straggling in-flight responses lift the ejection.
- `backends/regression-guards/circuit-breaker-record-success-gauge-leak.md` covers gauge balance only. No page justifies "a passive success ends an ejection".

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- The only `outlier_detection` diff is `fetch_update`→`try_update` in `allocate_policy_identity`; `store.rs:245-253` still runs `endpoint.record_success(); runtime.clear_outlier_ejected();` unconditionally inside `with_current_outlier_endpoint`, fenced only by policy identity.
- `state.rs:256` `record_success` and `coordinator.rs:467` `clear_outlier_ejected` are unchanged; success call sites remain at `pg_upstream_response_filter.rs:176`, `edgion_tcp.rs:278`, `tls/proxy.rs:400`, `tls/passthrough_proxy.rs:318`, with the token captured at `backends/mod.rs:531`.
