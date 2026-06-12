---
name: route-rules-inner-rwlock-fields-removed
description: Use when reviewing findings that `RouteRules` (or any similar single-write inside-an-ArcSwap container) wraps its inner fields in `parking_lot::RwLock` and pays unnecessary read-lock + deep-clone on every config-reload `Clone`; this is a real fix — the three inner fields are now plain `HashSet` / `Vec` because they are written once at construction and the outer `ArcSwap<DomainRouteRules>` already provides snapshot atomicity.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.
> Sibling case (same module, different lock primitive): [[domain-route-rules-inner-arcswap-removed]].

# `RouteRules` Inner `RwLock` Fields Removed (REAL FIX)

## Conclusion

`RouteRules` (`edgion-gateway/src/routes/http/routes_mgr.rs`) used to wrap
three fields in `parking_lot::RwLock`:

```rust
pub(crate) resource_keys: RwLock<HashSet<String>>,
pub(crate) route_rules_list: RwLock<Vec<Arc<HttpRouteRuleUnit>>>,
pub(crate) regex_routes: RwLock<Vec<Arc<HttpRouteRuleUnit>>>,
```

These fields were populated exactly once inside
`build_domain_route_rules_from_routes` (`conf_handler_impl.rs:478-484`) and
**never subsequently mutated**. A global grep across `src/` for `.write()` on
any of the three names returned zero results outside the construction point;
the only readers were:

- `RouteRules::clone` — three `RwLock::new(self.<field>.read().clone())`
  acquisitions per `Clone`, fired per-domain on every config reload.
- `tests.rs` — two `.read().len()` calls for count assertions.

Hot path `RouteRules::match_route` did **not** touch any of the three fields
— it reads only `match_engine` and `regex_routes_engine` (both
`Option<Arc<…>>`, no locking).

**Fix**: switch the three fields to plain `HashSet` / `Vec`. `Clone` becomes
a straightforward field `.clone()`. Construction at
`conf_handler_impl.rs:478-484` drops the `RwLock::new(…)` wrappers. The
outer `ArcSwap<DomainRouteRules>` on `HttpPortRouteManager.route_table`
continues to be the single snapshot-exchange point used by config reload and
by `pg_request_filter::load_route_table()` — hot-reload semantics unchanged.
Sibling gRPC `RouteRules` (`routes/grpc/routes_mgr.rs:25-28`) already used
plain `HashSet`/`Vec`; the HTTP fix aligns the two route subtrees.

## Why the Naive Form Was Wrong

- Treating "concurrent reader visibility" and "interior mutability primitive"
  as the same problem. A single-writer single-construction value held inside
  an `Arc<…>` that is itself swapped via outer `ArcSwap` already gives every
  reader a fully-published snapshot; inner `RwLock` adds no synchronization
  the language semantics did not already provide.
- Assuming `parking_lot::RwLock::read()` on an uncontended lock is free.
  It is cheap but still emits an atomic CAS + lock metadata word per field;
  on the `Clone` path it also blocks any (non-existent) future writer and
  inhibits compiler hoisting across the read guard.
- Allowing the same "kept for backward compatibility and debugging" doc
  comment that justified retaining the field (`regex_routes`) to also justify
  keeping the lock. The lock is not part of the back-compat contract — the
  field's *type shape* is. Plain `Vec<Arc<…>>` preserves the back-compat
  contract; the `RwLock<>` wrapper was always orthogonal.

## Pattern to Apply Elsewhere

For any struct whose fields are wrapped in `RwLock` / `Mutex` consider:

1. Grep for `.write(` / `.lock(` on each field name across the whole repo.
   If the only writes are at construction time, the lock is dead.
2. Is the struct held behind an outer `ArcSwap` / immutable `Arc`? If yes,
   inner locks add nothing — fresh outer snapshot publishes the inner state
   atomically.
3. When in doubt, look at the sibling protocol (gRPC vs HTTP, TCP vs UDP)
   for an established precedent. The Edgion routes subtree has consistently
   converged on "plain field inside ArcSwap snapshot" — diverging copies
   are usually historical, not load-bearing.
4. Do not "promote" a plain field to `RwLock` for future flexibility — if
   a real per-field hot-update appears later, that is the moment to add it,
   with the fresh use case dictating the right primitive.

## Reference

- FB-03 (`tasks/routes/routes-http-FB03-route-rules-dead-rwlock-fields.md`),
  closed 2026-05-28.
- Source: `edgion-gateway/src/routes/http/routes_mgr.rs` (field declarations,
  `Clone`), `edgion-gateway/src/routes/http/conf_handler_impl.rs`
  (construction at `build_domain_route_rules_from_routes`).
- Sibling already-plain reference:
  `edgion-gateway/src/routes/grpc/routes_mgr.rs:25-28` (`resource_keys`,
  `route_rules_list` as plain fields).
- Outer swap point preserved: `HttpPortRouteManager.route_table:
  ArcSwap<DomainRouteRules>`.
