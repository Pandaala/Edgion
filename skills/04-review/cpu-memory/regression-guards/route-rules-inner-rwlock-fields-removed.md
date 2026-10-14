---
name: route-rules-inner-rwlock-fields-removed
description: Use when reviewing findings that `RouteRules` (or any similar single-write inside-an-ArcSwap container) wraps its inner fields in `parking_lot::RwLock` and pays unnecessary read-lock + deep-clone on every config-reload `Clone`; this is a real fix — the three inner fields are now plain `HashSet` / `Vec` because they are written once at construction and the outer `ArcSwap<DomainRouteRules>` already provides snapshot atomicity.
knowledge_type: regression-guards
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
At the time of the HTTP fix, sibling gRPC `GrpcRouteRules` already used plain
`HashSet`/`Vec`. ARCH-045 subsequently removes those unused gRPC inventories;
the immutable outer-snapshot principle remains shared, but the fields differ.

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
- Current gRPC ownership: `edgion-gateway/src/routes/grpc/routes_mgr.rs`
  (`GrpcRouteRules` retains only its engine).
- Outer swap point preserved: `HttpPortRouteManager.route_table:
  ArcSwap<DomainRouteRules>`.

## Resolution references

- `routes-http-FB03` — 2026-05-28.
- `ARCH-045` — unused published gRPC inventories removed.

## gRPC published inventory ownership (ARCH-045)

`GrpcRouteRules` formerly retained a resource-name set and an unsorted unit
vector alongside the ranked inventory inside `GrpcMatchEngine`. Production
matching used the engine; rebuilding and statistics used manager caches. The
duplicate inventories therefore had no production reader requiring retention.
The builder now moves its collected vector into the engine. Failure diagnostics
keep scalar counts of nonempty input buckets and pre-filter input units, and
the failure helper still publishes a complete empty table.

The engine owns the published ranked inventory, not exclusive ownership of
the underlying unit Arcs: `route_units_cache` retains compiled units for reuse.
Outer ArcSwap publication, retained old snapshots, coherent ranked indexes,
hostname-tree reuse and cached runtime/selector identity remain unchanged.
`has_routes()` still means engine presence, including a successful build whose
nonempty inputs were all filtered out. HTTP's `patch_domain_route_rules` reads
its inventories for incremental patching, so their removal is not implied.

Regression coverage lives in the existing gRPC manager and configuration-handler
unit tests: fail-empty publication, empty versus filtered input, old snapshots,
retirement clearing and actual selected-unit identity against the compile cache.
Request-entry tests retain native gRPC, empty-table and gRPC-Web fallback cases.
These checks establish no measured heap-byte, latency or throughput gain and no
live HTTP/2 wire behavior. Re-evaluate if a real published-inventory consumer is
introduced; do not add a second stored inventory solely for inspection.
