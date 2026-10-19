# Evaluate shared OIDC introspection claims ownership

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `d486ade69`.

## Evidence and decision

In `edgion-gateway/src/plugins/http/openid_connect/token_verify.rs`, successful
introspection returns an owned serde_json::Value and cache_introspection_claims
clones that value into IntrospectionCacheEntry. The entry derives Clone in
plugin.rs. TinyUfoTtlCache::lookup_at in `edgion-gateway/src/cache/tiny_ufo.rs`
returns record.value.clone(), so a cache hit deep-copies the JSON tree. The cache
and the request currently own independent claims storage.

An immutable shared owner could avoid those deep copies. Merely changing the
cache field to Arc<Value> is insufficient if get_cached_introspection_claims still
clones the underlying Value to satisfy its current return type. verify_token,
JWT verification and introspection all currently return VerifyResult<Value>.
Choose between retaining that simple ownership, using shared results throughout,
or introducing an OIDC-local owned/shared result representation. Shared results
throughout add allocation and reference-counting costs to JWT and uncached paths;
a mixed representation keeps those paths owned but broadens result handling.

No candidate implementation or benchmark has run. Compare small and large claims,
cache-hit rates, misses, disabled caching, JWT-only traffic and concurrency before
choosing. This candidate concerns actual introspection-cache copies; accepted
ownership decisions for other credential consumers do not establish its outcome.

## Required ownership and cache boundaries

Keep the canonical TinyUfoTtlCache and positive-result admission policy. Do not
hold a cache lock or borrow a temporary cache record across request processing,
introduce a new cache, or change eviction policy to obtain a borrowed Value.
Preserve active checks, token-derived keys, configured/expiry-bounded TTL, disabled
cache behavior, generation isolation and authoritative results when insertion is
rejected or not retained. Hits still revalidate issuer/time claims and scopes.
Keep validation, dependency errors, fallback and header/context projection order.

LogicalWeight must continue accounting for the reachable JSON tree, including
string and container storage. Counting only an Arc pointer would invalidate the
logical byte budget. Review wrapper/record inline sizes and allocation allowances
with the canonical weight formula; disclose any resulting admission-boundary
change instead of silently weakening the budget. Logical weight is not exact RSS.

TTL bounds cache validity, not destruction: expired TinyUFO records can remain
until ordinary eviction or cache destruction. A shared request may keep claims
alive after eviction; the current request already keeps its independent copy.
Neither design guarantees immediate erasure at expiry. Compare total live storage
and release on completion/cancellation rather than claiming sharing alone creates
or fixes a secret-retention problem. Preserve redacted Debug and avoid claims,
tokens or cookie data in logs, labels and benchmark output.

## Acceptance for a later implementation

Use IMPL with an exact allow-list. Cover cache hit/miss, disabled cache, active and
inactive results, expired entries, rejected oversized admission and non-retention,
plugin generation replacement, concurrent readers and request cancellation.
Exercise JWKS-only and automatic fallback paths as well as introspection. Verify
unchanged JSON contents, validation and header/context output, and ensure callers
cannot mutate shared cached claims through the proposed result interface.

Measure allocation count/bytes, CPU, contention and peak/retained memory across
small/large/deep claims and concurrent hit/miss workloads. Include uncached/JWT
costs, cache weight boundaries, eviction and outstanding request owners. Retain
existing cache policy and response byte caps; this task grants no permission to
relax either or to rewrite the shared cache API for one consumer.

Fresh unchanged-code verification passed five tests selected by introspection_cache
covering entry shape, key stability, put/get, TTL calculation and automatic fallback.
They do not prove a shared-owner implementation correct or exhaust the acceptance
cases above. No benchmark ran. Original authority: AGENTS.md in-process cache
selection, `skills/04-review/security/review-evidence/credential-copy-ownership.md`
and `skills/04-review/oidc/regression-guards/dependency-error-projection.md`.
