# D0001 — Define Complete ExternalName DNS Cache Identity and Lifecycle

## Meta

- Status: fixed (2026-09-03)
- Severity: high
- Discovered in: B0035, `edgion-gateway/src/backends/discovery/dns_cache.rs`
- Category: correctness
- Resolution: [`skills/04-review/backends/regression-guards/externalname-dns-cache-identity-and-lifecycle.md`](../../../../skills/04-review/backends/regression-guards/externalname-dns-cache-identity-and-lifecycle.md)

## Evidence

The Gateway ExternalName DNS cache key contains the Service key and resolver fingerprint but not the queried `external_name`. Updating a Service to a different hostname under the same key and resolver can therefore reuse the previous hostname's addresses until TTL expiry. Service update and removal paths also do not invalidate cache entries, and expired entries for deleted unique Service keys have no guaranteed removal path.

## Why deferred

A safe fix requires one explicit ownership design across the request-path cache key, mutable Service generations, Service update/removal invalidation, and cache bounding. A target-only allocation or key tweak would freeze one policy without resolving lifecycle retention.

## Impact and open questions

- A renamed ExternalName may briefly route to the previous hostname's addresses.
- Churn across unique Service keys can retain stale cache entries for the Gateway process lifetime.
- Decide whether the canonical key includes hostname, Service generation, or both.
- Decide whether `ShardedLruCache` supplies the required explicit invalidation and replacement semantics.
- Preserve resolver precedence, configured TTL, and documented controller/gateway/per-request modes.

## Outcome

Fixed 2026-09-03. The cache key became the complete identity `(service_key, external_name, fingerprint)`, the store became a capacity-bounded `ShardedLruCache` (with a shard floor, since the global `lru_cache` default is a single shard), and the Service conf-handler invalidates removed Services after the store swap. A Service *update* needs no explicit invalidation: hostname and resolver are both in the key, so the previous generation's entry is unreachable by construction. See the linked resolution for the full decision record and the fix suggestions that were not accepted.

Concurrent miss coalescing, pending-query admission, cancellation ownership, and failure backoff were tracked separately and were **declined on 2026-09-03** without implementation: the default deployment (`mode=controller` in K8s conf_center) never reaches the self-resolve path, because the controller's DNS resolver loop publishes real EndpointSlices. See [`skills/04-review/backends/intentional-design/externalname-dns-miss-coalescing-not-implemented.md`](../../../../skills/04-review/backends/intentional-design/externalname-dns-miss-coalescing-not-implemented.md) for the trigger-surface analysis, the rejected designs, and the re-evaluation triggers.
