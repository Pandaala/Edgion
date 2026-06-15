---
name: redis-fail-open
description: Decision rule for findings about the Redis rate-limiting fail-open default behavior. Read this file first when reviewing "rate limiting is fully bypassed when Redis fails" findings.
---

# Redis Rate Limiting Fail-Open — NOT-AN-ISSUE

## Conclusion

`onRedisFailure` defaulting to `Allow` (fail-open) is classified as NOT-AN-ISSUE. Fix suggestions of the form "change default to `Deny`" or "fail-open is a security vulnerability" are not accepted.

> Note: both failure modes — (a) Redis detectably down (fast-eject, zero I/O via the `client.healthy()` pre-check) and (b) Redis healthy but slow/erroring (bounded wait, then give up) — route through the **single** `onRedisFailure` policy. There is no separate down-vs-slow policy knob (the classification boundary is racy, so a split would flap); the `unavailable`/`runtime` distinction exists only as a telemetry label, not a config decision.

## Core Rationale

**1. The configuration option exists; the operator can choose**

The `onRedisFailure: Deny` option is implemented and documented. For high-security scenarios that need fail-close, the operator can configure it explicitly. This is not a missing feature; it is an option already provided.

**2. fail-open vs fail-close is an operational tradeoff with no single correct answer**

- **fail-open (default)**: when Redis fails, the service continues; rate limiting is temporarily ineffective. For most business scenarios, "service available without rate limit" is preferable to "service unavailable"
- **fail-close**: when Redis fails, all requests covered by rate limiting are rejected — equivalent to actively causing a service outage

**3. Industry consistency**

The Kong Redis rate-limit plugin and the APISIX Redis rate-limit plugin both default to fail-open for the same reason: Redis is an optional auxiliary component; its failure should not cascade into a service outage.

**4. The bar for an attacker triggering a Redis failure is very high**

If an attacker can take down a Redis cluster (typically requires strong DDoS capability), they can already attack the Gateway directly; bypassing rate limiting is a secondary threat.

## Fix Suggestions Not Accepted

- Change the default to `Deny` (would unexpectedly interrupt service when Redis fails, affecting all routes that depend on rate limiting)
- Tag fail-open as a "security defect" (this is a documented design tradeoff)
- Force the operator to explicitly configure `onRedisFailure` (adds configuration burden with no value for most scenarios)
