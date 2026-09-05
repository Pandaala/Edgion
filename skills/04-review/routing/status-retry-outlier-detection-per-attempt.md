---
name: status-retry-outlier-detection-per-attempt
description: Use when reviewing how retry-on-status attempts contribute to passive outlier detection; every completed upstream response is classified exactly once before retry control flow returns.
status: fixed
finding_id: gw-http-proxy-resp-01
closed: 2026-08-02
---

# Status-Retry Outlier Detection Is Per Attempt — FIXED

## Conclusion

Every completed upstream response is classified for passive outlier detection
before retry-on-status can return. A retry chain therefore contributes one
sample per backend attempt, not one sample per client-visible request.

`HTTPStatus` errors used to drive retry or render a local status are control
flow, not transport failures, and must not also increment the local-origin
failure counter.

## Core Rationale

**1. Outlier detection measures backend attempts**

The backend produced a real HTTP response on each retry attempt. A matching
`retry.codes` entry changes whether that response is forwarded, but it does not
erase the backend observation. Counting only the terminal attempt delays
consecutive-failure ejection in direct proportion to the retry count.

**2. Classification must precede retry early returns**

`upstream_response_filter` stamps the attempt's status and immediately records
HTTP success or failure. This placement covers accepted retries, retry-budget
denial, and terminal responses uniformly while preserving the existing 1xx
early return for non-terminal informational responses.

**3. Error control flow must not double-count the observation**

Pingora passes an error returned by `upstream_response_filter` through
`error_while_proxy`. Status retry and retry-budget denial use `HTTPStatus`
errors there, so `error_while_proxy` excludes that error type from
`LocalOrigin`. Real read, write, timeout, reset, and protocol errors retain the
local-origin path.

## Operator Impact

Per-attempt counting is operator-visible: a rule with `retry.attempts: N` lets a
**single client request** contribute up to `N + 1` consecutive failures instead
of one. Each attempt re-runs *endpoint* selection (`ctx.selected_backend` is not
cleared between attempts, so the backendRef/Service pick is not re-run), which
spreads the samples across the endpoints actually picked — but when every
attempt lands on the same endpoint they all accumulate there. That is the case
for a single-endpoint Service, and for a request pinned by **strict** session
persistence; in the default non-strict mode `take_sp_target_or_select` consumes
`sp_target_addr` on the first attempt and the retry goes through plain LB
(`sp_should_repin_on_strict_retry` requires `strict`).

Local-origin failures have the same shape without any `retry` block:
`pg_fail_to_connect` falls back to `EdgionGatewayConfig.spec.maxRetries`
(default 3), so one client request can contribute up to 4 local-origin failures
against a default `consecutiveLocalOriginFailures` of 5.

`maxEjectionPercent` is not a backstop: the guard in
`outlier_detection/store.rs` uses `max_ejectable.max(1)`, so a single-endpoint
Service can always be ejected even at the default `maxEjectionPercent: 50`
(`0` skips the guard entirely). The gateway then has no endpoint to select, and
`get_peer`'s `_ =>` arm renders `end_response_503` — a 503, not a 502; 502 comes
only from `pg_fail_to_proxy` on an attempt that was actually made. The one
escape hatch is LB degrade mode: a non-zero `loadBalancer.degradeThreshold` makes
`selector::pick` bypass the health+ejection predicate, so the ejected sole
endpoint is still selected. Degrade mode defaults to `0` (disabled); it is kept
off the circuit-breaker user page on purpose (a per-feature page should not have
to teach a global LB knob) and is documented in
`docs/en/ops-guide/configuration/edgion-gateway-config.md` plus
`skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`.

Tuning rule: set `consecutiveErrors` / `consecutiveGatewayErrors` above
`attempts + 1` when one client request must not be able to trip ejection.
Documented for operators in
`docs/{en,zh-CN}/user-guide/http-route/backends/circuit-breaker.md`
("Interaction with Retries") and in
`skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`.

## Fix Suggestions Not Accepted

- Keep the outlier-detection hook after retry selection — accepted retries return before reaching it.
- Count only the terminal response — this changes the passive-ejection unit from backend attempt to client request.
- Move the HTTP classification earlier without filtering `HTTPStatus` in `error_while_proxy` — retry-budget denial can then count the same response again as a local-origin failure.
- Treat every error after a response header as non-local — mid-body transport failures remain genuine local-origin failures.

## Re-evaluation Triggers

Re-open only if:

- outlier detection is explicitly redesigned to aggregate by client-visible request rather than backend attempt; or
- Pingora stops routing response-filter errors through `error_while_proxy`.

## Reference Cases

- Finding `gw-http-proxy-resp-01`, fixed 2026-08-02.
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs`.
- `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs`.
- `skills/01-architecture/02-gateway/07-backends.md` failure-counting hooks.
