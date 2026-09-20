---
name: conf-handler-simple-map-pattern
description: Decide on "extract a generic SimpleMapConfHandler<T> for the RwLock<HashMap> add/update/remove ConfHandler pattern" findings. Closed as ⚠️ accepted tradeoff (infra-cluster-F012, 2026-05-28).
knowledge_type: intentional-design
---

# Generic `SimpleMapConfHandler<T>` Extraction — Declined

**Audit date**: 2026-05-28
**Linked issue**: infra-cluster-F012 (closed, ⚠️ accepted tradeoff, no code change)
**Conclusion**: Extracting a generic `SimpleMapConfHandler<T>` wrapper around the
`LazyLock<RwLock<HashMap<String, T>>>` + three-arm `add` / `update` / `remove` loop is
declined. Only two production handlers (`GatewayClassHandler`,
`EdgionGatewayConfigHandler`) actually implement this exact shape; the issue's
"routes subtree multiple copies" premise is wrong.

## The two real sites

- `edgion-gateway/src/config/gateway_class/conf_handler_impl.rs:57-104`
  (`GatewayClassHandler::partial_update`)
- `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs:67-116`
  (`EdgionGatewayConfigHandler::partial_update`, differs only by an
  `apply_link_sys_config(&config)` side-effect on the `add` and `update` paths)

Both follow the structurally identical pattern:

```rust
if !add.is_empty() {
    tracing::info!(count = add.len(), "<Kind> partial_update: added {} ...");
    for (key, r) in add {
        tracing::debug!(key = %key, name = %r.name_any(), "<Kind> added");
        // (EdgionGatewayConfig only) apply_link_sys_config(&r);
        store.insert(key, (*r).clone());
    }
}
// same shape for `update` (with `warn!` on key-miss) and `remove`
```

Each handler uses its own `LazyLock<RwLock<HashMap<String, T>>>` static store and
exposes its own `count_*` / `get_*_by_name` / `list_*` accessors. The two
`full_set` impls do call into the existing `arc_map_to_owned` helper in
`core/common/conf_sync/traits.rs`, so the trivially-shared part (Arc-map →
owned-map projection) is already factored out.

## Why the issue's "routes/* also duplicates this" premise is wrong

The `partial_update` bodies in `routes/{http,tcp,tls,udp,grpc}/conf_handler_impl.rs`
do **not** match the simple add/update/remove → store-insert pattern. They run a
multi-step pipeline (verified by reading `routes/http/conf_handler_impl.rs`):

1. Merge the incoming `Arc<HTTPRoute>` add/update maps and canonicalize
   semantically unchanged resources to the existing cache `Arc`.
2. Compute the affected `ListenerKey` set from both new routes and the cached old
   routes via `resolved_listeners_for_route`.
3. `self.route_cache.insert(...)` / `.remove(...)` (`DashMap`, not
   `RwLock<HashMap>`).
4. Rebuild only affected listener managers from the updated cache while holding
   the rebuild lock.
5. After the listener snapshot is complete, publish the HTTPRoute delta for
   plugin admission and update adaptive AI route contributions.

This is the documented "route ConfHandler" pattern (see
`route-architecture-consistency.md` in this directory): `Compute affected
listeners → update cache → rebuild_affected_listener_managers`. It is a different
algorithm, not a stylistic clone of the simple-map pattern. Plugins, backends,
and services handlers are likewise more complex (publication, policy
arbitration, listener recompute, runtime swaps).

## Why the extraction is declined

Applying the three-evidence bar from `size-and-similarity-heuristics.md`:

1. **Real duplication** — borderline. Two sites of ~48 lines each. The issue
   inflated the count by miscategorizing routes/plugins/backends; the actual
   dedup target is 2 handlers, not "many".
2. **Real coupling leak** — none. Each handler is self-contained; no cross-module
   import reaches into another handler's internals.
3. **Tracking cost decreases after splitting** — no. Each `partial_update` is a
   linear, locally-readable story. A generic `SimpleMapConfHandler<T>` would
   force readers of either handler to jump to a new `conf_sync` helper to see
   what `insert` / log fields look like, then back. With only two callers the
   indirection costs more reading than the duplication does.

Additional concerns specific to this extraction:

- Each handler keys its own `LazyLock<RwLock<HashMap<String, T>>>` static. A
  generic wrapper cannot hold the `static` itself (no static generics in stable
  Rust without `OnceLock<HashMap<...>>` per concrete type); the wrapper would
  have to take `&'static RwLock<HashMap<String, T>>` as a constructor argument,
  leaving the per-handler `static LazyLock`s in place — the same per-handler
  boilerplate the refactor was supposed to remove.
- The optional `on_add` / `on_update` hook signature has to accommodate
  `apply_link_sys_config(&config)` (which reads the *next* config before it
  enters the store); future hooks may want `&mut store`, `&old`, or
  fallible/async signatures. Choosing one hook shape today either over-fits
  EdgionGatewayConfig's needs or invents capability the simple cases don't use.
- The `tracing::info!` / `debug!` / `warn!` literal strings encode the kind name
  (`"GatewayClass added"`, `"EdgionGatewayConfig update: key not found ..."`).
  A generic wrapper either takes a kind-label parameter (every site repeats the
  literal anyway) or uses `std::any::type_name::<T>()` (changes log strings and
  couples production logs to compiler-internal names). The
  `stream-plugin-store-partial-update-per-key-debug-removed.md` precedent shows
  the project has already removed per-key `debug!` lines from one such handler;
  centralizing log shape now would either re-introduce that pattern or break the
  per-handler observability tuning that already happened elsewhere.

The accepted-tradeoff posture matches the established class:
`link-sys-F008` / `link-sys-F013` (don't unify `dispatch_full_set` /
`dispatch_partial_update`), `style-consolidation-thresholds.md` (duplication
itself is not a defect), `plugins-runtime-F003` (don't extract
`add_plugins_inner` even with four sites). Refactor declined; pure no-behavior
change DRY with no real maintenance evidence and no measurable runtime impact.

## Re-evaluate only on

- A **third** ConfHandler<T> impl using the exact `LazyLock<RwLock<HashMap>>` +
  three-arm `add` / `update` / `remove` shape appears (routes/plugins/backends do
  not qualify — they have additional pipeline steps).
- The two handlers' log field schemas (`count`, `key`, `name`) genuinely drift in
  a way that breaks a production observability query — drift-prevention is the
  only acceptance gate per `style-consolidation-thresholds.md`.
- `ConfHandler<T>` itself gains a new mandatory method (e.g.
  `key_diff(&self, ...) -> ...`) forcing both handlers to re-implement the same
  adapter code.

Re-reports of the "extract `SimpleMapConfHandler<T>`" / "fold partial_update into
a generic wrapper with `on_add` / `on_update` hooks" / "this pattern is also
copied across routes/plugins/backends" form are not accepted; verify the actual
shape of routes/* and the absence of a third LazyLock-RwLock-HashMap simple-map
handler before re-reporting.

## Resolution references

- `infra-cluster-F012` — 2026-05-28 (⚠️ accepted tradeoff).
