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
