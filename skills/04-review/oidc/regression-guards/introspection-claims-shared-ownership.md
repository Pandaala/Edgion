---
name: oidc-introspection-claims-shared-ownership
description: Use when reviewing OpenID Connect introspection cache hits, claims cloning, `IntrospectionCacheEntry` ownership, or its TinyUFO logical weight.
status: fixed
finding_id: oidc-05
closed: 2026-11-06
knowledge_type: regression-guards
---

# OIDC introspection cache shares claims instead of deep-cloning them

## Conclusion and scope

`IntrospectionCacheEntry` (`edgion-gateway/src/plugins/http/openid_connect/plugin.rs`)
stores `claims: Arc<Value>`. `TinyUfoTtlCache::lookup_at` still returns a clone of
the entry, but that clone is now one reference-count increment instead of a
fresh `serde_json::Value` tree. `verify_token_via_introspection` and
`verify_token` return `VerifyResult<Arc<Value>>`; a miss moves the parsed
introspection `Value` into one `Arc` and inserts `Arc::clone` of it. JWT
verification helpers still return an owned `Value`, and `verify_token` wraps
it once. Consumers (`validate_claims_from_value`, `validate_required_scopes`,
`apply_upstream_headers`) borrow `&Value` through deref coercion.

Scope: Gateway `openid_connect` plugin only. The generic TinyUFO cache API is
unchanged.

## Rationale and evidence

Before the fix, every authenticated request that hit the introspection cache
deep-copied the whole claims JSON (every map, string and array), and each
insert cloned it again. A hit performs no network IO, so that copy dominated
the per-request authentication work on this path while no consumer needed an
owned tree.

## Alternatives and limits

- Preserved: canonical `TinyUfoTtlCache`, positive-result-only admission,
  token-digest keys, TTL bounded by `exp`, disabled cache when TTL is zero,
  re-validation of claims and required scopes on every hit, and use of the
  authoritative introspection result when insertion is not retained.
- Byte budget: `IntrospectionCacheEntry::logical_weight` counts the root
  `Value`, the two `Arc` reference counts and the full reachable JSON tree,
  because those now live in the shared heap allocation rather than inline in
  the record. The budget is therefore not reduced by the `Arc` indirection.
- Lifetime: a request holding the shared `Arc` keeps the claims alive until
  the request drops it, the same bound as the former per-request clone. Cache
  TTL bounds validity, not destruction, as recorded in
  [credential copy ownership](../../security/review-evidence/credential-copy-ownership.md).
- Rejected: changing the generic cache to return borrowed records, which
  would hold a cache borrow across request processing.

## Verification and re-evaluation

`cargo test -p edgion-gateway openid_connect` passed on 2026-11-06, including
`introspection_cache_hits_share_claims_without_reducing_weight` (pointer-equal
consecutive hits and weight covering the root value plus tree). Re-evaluate if
the cache entry gains other owned fields, if `TinyUfoTtlCache` stops cloning
values on lookup, or if claims become mutated after verification.

## Resolution references

- oidc-05 (historical path `tasks/todo/oidc-05-introspection-claims-ownership.md`, P3; task file removed on closure), fixed 2026-11-06.
