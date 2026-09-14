---
name: oidc-discovery-failure-backoff
description: Use when reviewing OpenID Connect discovery stampede, failed-fetch coalescing, Shared-future, or cancellation-ownership findings; covers why a short failure backoff is the accepted fix and why a cancellation-safe shared fetch is not.
status: fixed
finding_id: D0059
closed: 2026-09-05
knowledge_type: regression-guards
---

# OpenID Connect Discovery Failure Backoff — FIXED

## Conclusion

A failed `JwksProvider::get_or_fetch_discovery` publishes a short bounded
failure outcome. Concurrent or immediately following cold callers reuse that
typed error until the deadline; the next call after the deadline retries, and a
successful document publishes one new `Arc` snapshot. Background prewarm
already exists and is not a second feature.

Fix suggestions of the form "share one cancellation-safe in-flight Future so
dropping the initiating request keeps the HTTP fetch alive", "add a
`rediscoveryLifetime` CRD field", and "negative-cache discovery failures
forever" are **not accepted**.

## Core Rationale

**1. The defect was the missing failure publication, not the mutex**

The discovery refresh mutex already serializes one in-flight fetch and
double-checks the success cache. After a failure the cache stayed empty, so
each waiter started another provider timeout. Recording `(deadline, error)`
after the failed attempt is the origin fix.

**2. Envoy and Kong stop at a short rediscovery gate**

Envoy has no OIDC discovery. Its JWKS `async_fetch` retries failed background
fetches after `failed_refetch_duration` (default 1s) and cancels on-demand
fetches when the request is destroyed. Kong's OpenID Connect plugin uses
`rediscovery_lifetime` (30s, 5s after a failed discovery) plus stale fallback.
Neither implements a Shared-future cancellation protocol. Edgion already has
constructor prewarm (`spawn_discovery_prewarm`) and JWKS `min_refresh`
backoff; discovery now uses a fixed 5-second failure gate, matching Kong's
failed-discovery lifetime.

**3. Leader cancellation remains structural**

If the lock holder is cancelled, the mutex guard drops and the next waiter
re-checks cache and backoff, then fetches. That is the same shape already
accepted for JWKS and recorded as the proportionate alternative to
`Shared<Future>` in
[externalname-dns-miss-coalescing-not-implemented.md](../../backends/intentional-design/externalname-dns-miss-coalescing-not-implemented.md).

## Fix Suggestions Not Accepted

- **Cancellation-safe shared in-flight Future** — beyond Envoy on-demand JWKS
  and Kong rediscovery; adds registry lifecycle and cancellation propagation
  for a cold-only path
- **Operator `rediscoveryLifetime` / `discoveryFailureBackoff` field** — a
  fixed 5-second gate needs no CRD; a knob would expand schema and docs for
  no proven tuning need
- **Permanent negative cache** — would poison recovery after a transient IdP
  outage or validator failure
- **A second prewarm implementation** — `spawn_discovery_prewarm` already
  retries for about five minutes on the background task
- **Changing frozen OpenID Connect integration tests** — provider-local
  Tokio fixtures cover backoff, reuse, and recovery

## Re-evaluation Triggers

Re-open this decision only if:

- Discovery gains a TTL or periodic refresh so failure backoff must compose
  with an expiry state machine
- A production incident shows request cancellation of the mutex holder
  dropping enough in-flight fetches to matter after the 5-second gate
- The stored failure classification changes again. D0058's
  [dependency error projection](dependency-error-projection.md) triggered this
  review: the cached error becomes data-free while the 5-second deadline and
  recovery policy remain unchanged.

## Reference Cases

- D0059 — `tasks/todo/openid-connect-discovery-failure-coalescing.md`
- Source: `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs`
- Sibling JWKS backoff: `get_jwks` `min_refresh` + stale-while-revalidate
- Prewarm: `spawn_discovery_prewarm` in `plugin.rs`

## Resolution references

- `D0059` — 2026-09-05.
