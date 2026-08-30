# D0001 — Define Complete ExternalName DNS Cache Identity and Lifecycle

## Meta

- Status: needs-discussion
- Severity: high
- Discovered in: B0035, `edgion-gateway/src/backends/discovery/dns_cache.rs`
- Category: correctness

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

## Candidate next step

Design the cache key and invalidation contract together with the Service conf-handler and backend consumer. Add update, delete, TTL, resolver-change, and bounded-growth regressions before replacing the current cache.
