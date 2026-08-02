---
name: oidc-access-token-cache-ttl-only
description: Use when reviewing OIDC access-token cache expiry storage; records that expiry belongs only to the cache TTL and must not be duplicated in each value.
status: fixed
finding_id: gw-plugin-oidc-04
closed: 2026-08-02
---

# OIDC Access-Token Cache Stores Expiry Only as TTL — FIXED

## Conclusion

`AccessTokenCacheEntry` stores only the token. The existing cache-lifetime policy is fully represented by the `ShardedLruCache` TTL computed at insertion; removing the duplicate absolute timestamp does not change that policy.

Fix suggestions that add an `expires_at` field without a distinct consumer are **not accepted**.

## Core Rationale

**1. The cache already owns expiration**

`cache_access_token` derives a bounded `Duration` from the optional token expiry, configured clock skew, and existing fallback policy, then passes it to `ShardedLruCache::insert`. The cache rejects entries after that TTL from `get`, so a second timestamp in the value does not affect lifetime.

**2. No independent expiry decision needs the timestamp**

The only value consumer, `get_cached_access_token`, returns the token after the cache has applied TTL expiry. Refresh decisions use the session payload rather than `AccessTokenCacheEntry`.

## Fix Suggestions Not Accepted

- Restore `expires_at` for possible future use — speculative storage should be added only with a real consumer.
- Add a manual expiry check in `get_cached_access_token` — this duplicates the cache's TTL policy and creates two expiry authorities.

## Re-evaluation Triggers

Re-open this decision only if a cache-entry consumer needs the original absolute expiry for behavior that cannot be derived from the cache TTL or session payload.

## Reference Cases

- `gw-plugin-oidc-04` (fixed 2026-08-02).
- `edgion-gateway/src/plugins/http/openid_connect/token_exchange.rs`.
- `edgion-gateway/src/cache/lru.rs`.
