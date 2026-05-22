---
name: bandwidth-limit-per-stream
description: Decision rule for findings about BandwidthLimit plugin lacking per-connection token bucket. Read this file first when reviewing "BandwidthLimit is stateless / no per-connection state / chunk_size / rate_bps is not a real rate limit" findings.
---

# BandwidthLimit Per-Response-Stream Scope — DOCUMENTED, NOT-AN-ISSUE

## Conclusion

The `BandwidthLimit` HTTP plugin (`src/core/gateway/plugins/http/bandwidth_limit/plugin.rs`) deliberately
operates per response stream: each upstream body chunk yields `delay = chunk_size / rate_bps`, Pingora
applies that delay before sending the next chunk, and there is **no per-connection token bucket, no
wall-clock accounting, and no cross-stream coordination**. Fix suggestions of the form

- "add per-connection token bucket via `ctx`"
- "should accumulate wall-clock time elapsed between chunks"
- "stateless chunk-size division is not a real rate limiter"

are **not accepted** as code changes. The behavior is documented in:

- `docs/en/user-guide/http-route/filters/edgion-plugins/bandwidth-limit.md` → Important Notes §3
- `docs/zh-CN/...` mirror page
- `plugin.rs` file-header `## Scope` section

If a future audit re-reports the missing token bucket, point the reviewer at this file and the docs.

## Core Rationale

**1. Scope matches the user-visible promise.** The `rate` field caps a single download's throughput,
which is the dominant use case (preventing one large response from saturating a link). Operators
configuring `rate: 1mb` on a `/download/*` route expect each download to be capped, not the entire
gateway.

**2. Stateless = zero coordination cost.** A real per-connection token bucket would need either
per-`ctx` state (allocated for every request, paid for by every request that *doesn't* use this
plugin too, depending on placement) or a `Mutex`/`RwLock` across the connection's filter calls. For a
P3 feature this is a poor tradeoff.

**3. HTTP/2 multiplexing aggregation is not the gateway's job.** Global / per-connection aggregate
caps belong at L4 (LB-level QoS, kernel `tc`, service-mesh ingress policy). Mixing aggregate
enforcement into a per-request body filter would couple unrelated streams and complicate the
return-`Option<Duration>` Pingora contract.

**4. The Pingora contract favours per-call decisions.** `upstream_response_body_filter` is invoked
per chunk per response, and its only side effect is the returned delay. Threading a per-connection
token bucket through the filter would need additional plumbing for refill, race-free updates, and
end-of-stream cleanup — all for marginal benefit.

## What the docs say (mandatory cross-reference)

- The "Important Notes" section explicitly names the scope as **per response stream**, calls out the
  HTTP/2 multiplexing implication (N streams × `rate`), and directs operators to external L4
  mechanisms for aggregate caps.
- Future doc changes that re-introduce "per-connection" wording are regressions; this file is the
  source of truth for the design intent.

## Fix Suggestions Not Accepted

- Add `ctx`-stored token bucket to enforce per-connection / per-request aggregate (changes behavior,
  P3 → no value justification)
- Track wall-clock time elapsed between chunks and adjust delay (more complex math for an effect
  Pingora's natural pacing already produces)
- Tag the current behavior as "fake rate limiter" or P1/P2 in future audits (the design is
  documented and matches user expectations)
- Change the default to "per-connection" by allocating per-`ctx` state by default (silent behavior
  change, breaks existing deployments)

## Related

- Issue P3-g4-007 (closed via doc clarification, 2026-05-15)
