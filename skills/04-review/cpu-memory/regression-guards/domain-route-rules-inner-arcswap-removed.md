---
name: domain-route-rules-inner-arcswap-removed
description: DomainRouteRules keeps immutable inner fields under one outer atomic snapshot; do not add per-field ArcSwap loads or independently published generations.
knowledge_type: regression-guards
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `DomainRouteRules` Inner `ArcSwap` Fields Removed (REAL FIX)

## Conclusion

`DomainRouteRules` (`edgion-gateway/src/routes/http/routes_mgr.rs`) used to wrap
its three routing components in individual `ArcSwap` / `ArcSwapOption` fields:

```rust
pub(crate) exact_domain_map: ArcSwap<HashMap<DomainStr, Arc<RouteRules>>>,
pub(crate) wildcard_engine: ArcSwapOption<RadixHostMatchEngine<RouteRules>>,
pub(crate) catch_all_routes: ArcSwapOption<RouteRules>,
```

These fields were populated exactly once inside
`build_domain_route_rules_from_routes` and **never subsequently stored or
swapped** — a global grep for `.store(` / `.swap(` on any of the three names
returned zero results. Config reload rebuilds a fresh `DomainRouteRules` and
swaps the **outer** `ArcSwap<DomainRouteRules>` on
`HttpPortRouteManager.route_table`; the inner `ArcSwap`s never see the second
write.

Every HTTP request paid 3 extra atomic loads in `DomainRouteRules::match_route`
(`exact_domain_map.load()`, `wildcard_engine.load()`,
`catch_all_routes.load()`) that produced no concurrency benefit — the
`DomainRouteRules` instance is already held exclusively by the outer `ArcSwap`
guard. Pure cache-line pressure on the request hot path.

**Fix**: switch the three fields to plain `Arc` / `Option<Arc<…>>`. All
callers that previously called `.load()` now access the field directly. The
outer `ArcSwap<DomainRouteRules>` on `HttpPortRouteManager.route_table`
continues to be the single atomic swap point used by config reload and by
`pg_request_filter`'s `load_route_table()` — hot-reload semantics are
unchanged.

## Why the Naive Form Was Wrong

Current shape after Route hostname unification: `DomainRouteRules` embeds the
gateway-owned immutable `HostnameMatcher<RouteRules>` plus a separate hostless
group. The historical three-field examples above describe the original repair,
not the current struct layout. Its frozen hostname tree and persistent contents
are paired inside the same outer table; there is still only one owner-level
ArcSwap and no independently mutable per-host publication. Sharing an unchanged
tree or payload Arc across generations does not weaken that invariant.
See the [hostname algorithm contract](../../../12-algorithms/route-hostname-matching.md).

- Treating "snapshot replaced atomically" and "field swap-in-place" as
  interchangeable design points. The data-plane reader needs an atomic
  *snapshot* of the routing table; the writer needs an atomic *swap* of one
  snapshot for another. Both are satisfied by **one** `ArcSwap` at the right
  layer. Nesting `ArcSwap`s pays the atomic cost N times for one logical swap.
- Assuming a `.load()` on a never-stored `ArcSwap` is free. It still emits the
  `SeqCst`-equivalent atomic read on every call and prevents the compiler from
  hoisting / merging subsequent reads of the inner pointer.

## Pattern to Apply Elsewhere

For any struct whose fields are wrapped in `ArcSwap*` consider:

1. Where does the swap actually happen? Grep `.store(` / `.swap(` on each
   field name. If the only writes are at construction time, the wrapper is
   dead — replace with `Arc` / `Option<Arc<…>>`.
2. Is the outer container already behind an `ArcSwap`? If yes, *any* inner
   `ArcSwap` is structurally redundant for the swap-granularity argument — a
   fresh outer snapshot already gives the data-plane reader a consistent inner
   view. Keep inner `ArcSwap` only when there is a real per-field hot-update
   path that must avoid rebuilding the whole snapshot.
3. Apply the cost only at the layer that needs it. Conversely, do not
   "promote" an inner field to `ArcSwap` "for future flexibility" — if a
   real per-field hot-update appears later, that is the moment to add it,
   with the fresh use case dictating the right granularity.

## Reference

- F-003 (`tasks/routes/routes-http-F003-domain-route-rules-inner-arcswap-waste.md`),
  closed 2026-05-28.
- Source: `edgion-gateway/src/routes/http/routes_mgr.rs` (field declarations,
  `match_route`, `stats`), `edgion-gateway/src/routes/http/conf_handler_impl.rs`
  (`build_domain_route_rules_from_routes` construction).
- Outer swap point preserved: `HttpPortRouteManager.route_table:
  ArcSwap<DomainRouteRules>` — the single atomic snapshot exchange used by
  `pg_request_filter::load_route_table()` and config reload.

## Resolution references

- `routes-http-F003` — 2026-05-28.
