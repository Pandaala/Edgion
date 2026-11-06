# gw-plugrt-01: Every publisher input event flushes the whole response cache and recomposes every scope

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: large-direction
- **Location**: `edgion-gateway/src/plugins/publication/publisher.rs:158-166` (`change`), `:196-337` (all input callbacks), `:353-356` (`set_http` no-op return), `:385-414` (`update_gateways` unchanged check), `:416-506` (`scope_plan`), `:508-730` (`compose`, esp. `:520`, `:532`, `:626-631`, `:680-728`); `edgion-gateway/src/response_cache/storage.rs:240-258` (`Store::advance_epoch`, `entries.clear()` at `:250`), `:628-668` (key already frames runtime identities)

## Summary
`PluginPublisher::change` unconditionally recomposes the entire publication and calls
`response_cache::advance_epoch()`, which clears every cached response in the process. This happens for
every delivered input event of any kind (HTTPRoute, GRPCRoute, TCP/UDP/TLS routes, Gateway, GatewayClass,
EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins), in any namespace, including events that the
publisher itself already recognised as no-ops. Composition is also O(world) per event rather than
proportional to the change.

## Current behavior (evidence)
- `change()` (`publisher.rs:158-166`) always runs `state.compose(&previous)` and
  `next.stamp_cache_epoch(crate::response_cache::advance_epoch())`. There is no "nothing changed" early return.
- No-op detection exists but is ignored for the epoch: `set_http` returns early when `same_source` is true
  (`:353-356`), `update_gateways` skips unchanged Gateways (`:385-414`), `replace_sources` reuses identical
  Arcs (`:98-105`); `change()` still advances the epoch afterwards. A conf-sync relist (`full_set`) of an
  unchanged world therefore empties the cache.
- `Store::advance_epoch` (`storage.rs:240-258`) deactivates the old epoch and calls `self.entries.clear()`
  — a global flush, not a per-scope or per-route invalidation.
- The cache key already frames the identities that a relevant change would alter: route `resource_key`,
  rule id, `cache_identity()` of the global, gateway, route and every ExtensionRef `PluginRuntime`
  (`storage.rs:628-668`; `PluginRuntime::new` assigns a fresh identity per compiled view,
  `plugin_runtime.rs:312-317`). A change to an unrelated route or plugin cannot make a stored entry stale.
- Cost per event: `scope_plan` iterates **all** routes for every GatewayClass (`:464`); the `routes`
  signature is a list of `Arc` pointers (`:501`), so any route event makes the class signature differ
  (`:532`) and `rule_admission` is recomputed for every route of that class (`:626-631`);
  `http_sources` is cloned (`:520`); the inventory re-formats and sorts a `String` for every generation and
  cached entry (`:680-728`). Route handlers call `update_http_routes`/`update_grpc_routes` on every batch.

The behavior is documented in `docs/en/user-guide/http-route/filters/edgion-plugins/response-cache.md`
("Configuration publication broadly invalidates visible entries"), but the documentation does not state that
no-op and unrelated events also flush, and the key-level identities make the global flush unnecessary.

## Industry comparison
- NGINX `proxy_cache` survives `nginx -s reload`; invalidation is per key (purge) or by TTL.
- Varnish/Fastly use ban/purge expressions scoped to the affected objects, not a global wipe on config load.
- Envoy xDS only rebuilds the resources whose version changed (delta xDS / per-resource versions); an
  unrelated RDS update does not reset state owned by other routes. Envoy's cache filter is keyed per route.

## Impact
- In a busy multi-tenant cluster (CI deploys, Argo sync, HPA-driven route churn, controller relists),
  the response cache hit rate collapses to near zero: one tenant's HTTPRoute edit empties every tenant's
  cache, producing an origin load spike at exactly the moment config churns.
- Gateway reconnect/relist (`full_set`) of an unchanged configuration also flushes.
- O(routes × classes) recomposition plus O(E log E) inventory string building per event, under the
  publisher mutex, on the conf-sync thread.

## Suggested direction
1. Make `change()` detect a no-op: compare the composed scopes/listener views by `Arc::ptr_eq` with the
   previous publication (compose already reuses Arcs when nothing changed) and skip both the store and the
   epoch advance when everything is pointer-identical.
2. Stop using the global epoch as the invalidation mechanism for ordinary config changes. The key already
   carries runtime/route identities; keep the epoch only for inputs not represented in the key (or make it
   per-GatewayClass scope), so an unrelated change leaves other entries valid. Stale entries keyed by
   retired identities simply age out by LRU/TTL.
3. Make composition incremental: key the route signature by a per-class route-set version rather than a
   pointer list, recompute `rule_admission` only for changed routes, and build the debug inventory lazily
   (on admin request) instead of on every event.

Recommendation: (1) immediately (small, safe), then (2); (3) when route counts grow.

## Notes
- Checked `skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md` ("Publisher
  retirement atomically clears visibility") and `plugin-runtime/regression-guards/scoped-policy-runtime-publication.md`
  (accepts O(listener count) per delta). Neither addresses no-op events, unrelated-namespace flushes or the
  redundancy with key identities; this is new evidence against the blanket flush, not a re-report.
- Cross-unit observation for the response-cache reviewer (not filed here): the same key frames
  `remote_ip`, `downstream_peer_ip` and `X-Forwarded-For` unconditionally (`storage.rs:690-693`), making the
  cache effectively per client IP.

## Related findings

- gw-infra-05 (response-cache hit-ratio analysis; this finding is one of its causes).

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/plugins/publication/publisher.rs:158-165`: `change()` always composes and calls `stamp_cache_epoch(advance_epoch())`, with no no-op return even after `set_http` (`:353-356`) or `replace_sources` (`:98-105`) detect unchanged input.
- `edgion-gateway/src/response_cache/storage.rs:240-258`: `advance_epoch` deactivates the epoch and calls `self.entries.clear()`, a process-wide flush.
- The documented broad invalidation and `skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md` cover publication retirement, not flushes on no-op/unrelated events; that is the new evidence. Step (2) (dropping the epoch flush) needs care for inputs not represented in the key, as the finding notes; step (1) is safe.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `edgion-gateway/src/plugins/publication/publisher.rs` has no diff; `change()` still composes and advances the cache epoch unconditionally.
- `edgion-gateway/src/response_cache/storage.rs:240-258`: `advance_epoch` still clears entries; the only diff is `fetch_update` -> `try_update` (same lines).
