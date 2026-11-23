# gw-infra-05: Response cache is structurally limited to near-zero hit ratio (global flush on every config publish, 64-entry ceiling, worst-case slot charging, no stampede control)

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: large-direction
- **Location**: `edgion-gateway/src/plugins/publication/publisher.rs:159-164` (`PluginPublisher::change`), `response_cache/storage.rs:47-52,240-258` (`advance_epoch`), `storage.rs:432-490` (`Pending::create`), `cli/config.rs:258-322` (`ResponseCacheLimits` default/`validate`), `response_cache/policy.rs:22-34,70-104,106-130` (`request_is_eligible`, `public_max_age`, `supported_header_lifetime`), `response_cache/lifecycle.rs:62-74` (`prepare_hit`)

## Summary
The response cache has strict correctness bounds but cannot work as a cache at scale.
(1) Every plugin publication clears the whole process cache. That includes any Gateway, HTTPRoute, GRPCRoute,
stream route, EdgionPlugins, GatewayClass or EdgionGatewayConfig change anywhere in the cluster.
(2) The configured defaults are also hard maxima: 64 entries, 16 shards, 128 MiB.
(3) Each entry is charged and allocated at the full 1 MiB body cap plus 16 KiB, regardless of its real size.
(4) There is no request coalescing, stale serving or revalidation.
(5) Only the exact `Cache-Control: public, max-age=N` form is cacheable.
Together these give a cache that holds at most 64 objects, is wiped by unrelated tenants' config changes,
and lets every miss go to origin concurrently.

## Current behavior (evidence)
- `PluginPublisher::change` (called by every `replace_*`/`update_*` source callback, `publisher.rs:167-330`)
  calls `crate::response_cache::advance_epoch()`. `Store::advance_epoch` deactivates the old epoch and runs
  `self.entries.clear()` (`storage.rs:224`). The key already includes route, discovery, traffic, TLS and
  policy stamps (`response_cache/key.rs`), so entries whose inputs did not change are discarded anyway.
- `ResponseCacheLimits::validate` rejects any value above `Self::default()`: `max_entries > 64`,
  `total_bytes > 128 MiB` and so on (`cli/config.rs:283-289`). Operators cannot enlarge the cache.
- `Pending::create` leases `body_cap + metadata_cap + 1024` (about 1.04 MiB at defaults) from the total
  ledger and the 16 MiB pending ledger, then `try_reserve_exact(store.body_cap)` and
  `try_reserve_exact(store.metadata_cap)`. The stored `Entry` keeps that capacity, and `decode` checks
  `capacity()`. A 2 KiB JSON response costs about 1.04 MiB, and at most about 15 misses can fill concurrently.
  `validate` requires `max_entries x (body+metadata+1024) + pending <= total_bytes`, which is why 64 is the
  ceiling.
- No single-flight: `prepare_hit` -> `lookup` miss -> every concurrent request is forwarded and each tries to
  admit its own pending fill. After a flush (see above) a hot key sends N concurrent requests to origin.
- `public_max_age` accepts exactly two directives, `public` and `max-age=N` in either order. Common origin
  headers such as `public, max-age=300, s-maxage=600`, `max-age=60, public, stale-while-revalidate=30` and
  `s-maxage=…` are not cached. Any request `Cache-Control`/`Pragma` disables the lookup
  (`request_is_eligible`). There is no conditional revalidation of stale entries.

## Industry comparison
- NGINX `proxy_cache`: millions of keys per `keys_zone`, body on disk with `max_size` accounting by real
  size, `proxy_cache_lock` (single-flight), `proxy_cache_use_stale updating|error|timeout`,
  `proxy_cache_background_update`, revalidation with `If-None-Match`, and honors `s-maxage`, `Expires` and
  `X-Accel-Expires`. Configuration reloads do not purge the cache.
- Varnish: request coalescing (waiting list), grace mode (stale-while-revalidate), and object size accounted
  by real size.
- Kong `proxy-cache`: memory strategy sized by real body size in a shared dict, per-plugin
  `cache_ttl` / `cache_control` honoring, and an Admin API purge by key or plugin. Config changes do not flush.
- Envoy `cache` filter (`SimpleHttpCache`): RFC 9111 directive parsing, validation, and no flush on xDS
  updates; the key/route identity makes stale reuse impossible.

## Impact
The feature carries a lot of correctness machinery (epochs, stamps, seals, leases) but provides little origin
offload in any cluster with ordinary config churn. Users who enable it can also see synchronized cache-miss
stampedes after every unrelated config push, so enabling the cache can make origin load spikier than not
caching.

## Suggested direction
Options, recommended order:
1. **Scope invalidation to the key, not the process.** The key already embeds route, policy, discovery and TLS
   identities, so an unrelated publication cannot produce a wrong hit. Stop calling `entries.clear()` on
   `advance_epoch`, or advance per-route epochs, and let mismatched keys age out through the LRU and TTL. Keep
   the global fence only for changes that alter key derivation itself.
2. **Charge real size.** After EOS, shrink the arena (`shrink_to_fit`, or copy into an exact-size `Bytes`)
   and convert the lease to the actual body plus metadata length. Keep the cap only as an admission bound for
   pending fills. Make `max_entries`/`total_bytes` operator-raisable within a sane global ceiling instead of
   default == max.
3. **Single-flight per key.** Keep a bounded in-flight map keyed by cache key. Followers wait, bounded by the
   request deadline, for the leader's publication, or fall through to origin on leader failure, as with NGINX
   `proxy_cache_lock`/`lock_timeout`.
4. **Directive grammar.** Parse `Cache-Control` as a directive list per RFC 9111. Accept `s-maxage`
   (preferred for shared caches), ignore unknown extensions, and keep rejecting `private`, `no-store` and
   `no-cache`. Optionally add `stale-while-revalidate` with background refresh.
If the product decision is that the cache stays a narrow micro-cache, document that explicitly and remove the
configuration surface that suggests otherwise.

## Notes
Checked `skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md`. It accepts the
private preadmitted arena, capacity-based ledger charge (to avoid memset), the defaults, and that "Publisher
retirement atomically clears visibility". It treats these as safety properties, but it does not consider hit
ratio, the default==max ceiling, or the absence of stampede protection. Shrinking after EOS keeps the
"no memset / no reallocation during push" property that page protects. Not tracked in `tasks/`.
The separate [Gateway memory-pressure shedding](../todo/gateway-overload-manager.md) task covers aggregate process pressure.

## Related findings

- gw-plugrt-01 (root cause of the global flush: publisher advance_epoch clears the cache on every input event). Also: the cache key includes remote_ip / downstream peer IP / X-Forwarded-For (response_cache/storage.rs:690-693), making the cache effectively per client IP.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged. The global-flush root cause overlaps gw-plugrt-01 (another batch). Keep this finding for the capacity, charging, stampede and directive-grammar aspects.
- `publisher.rs:158-164` `change` calls `response_cache::advance_epoch()` on every input edit, and `storage.rs:240-253` `Store::advance_epoch` runs `self.entries.clear()`.
- `cli/config.rs` `ResponseCacheLimits::validate` rejects any field above `Self::default()` (`max_entries > 64`, `total_bytes > 128 MiB`, ...), so the defaults are hard maxima.
- `policy.rs:70-93` `public_max_age` accepts exactly two directives (`public` and `max-age=N`). `s-maxage` and any third directive make a response uncacheable.
- The user doc (`docs/en/user-guide/http-route/filters/edgion-plugins/response-cache.md`) calls these values "defaults" and does not position the feature as a deliberately narrow micro-cache. `http-body/regression-guards/bounded-response-cache-capture.md` covers safety, not hit ratio.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `response_cache/` diff is only `fetch_update`→`try_update`; `Store::advance_epoch` (`storage.rs:240-258`) still runs `self.entries.clear()` and `publisher.rs:163` still advances the epoch on every change.
- `cli/config.rs:272-291` `ResponseCacheLimits::validate` still rejects any value above `Self::default()` (only the YAML crate changed in that file); `policy.rs:70` `public_max_age` and `lifecycle.rs:62` `prepare_hit` are unchanged (no single-flight, no `s-maxage`).
