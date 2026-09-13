---
name: l4-rebuild-helper-clear-pass
description: Use when reviewing TCP / TLS / UDP `Global*RouteManagers` `rebuild_all_listener_managers` or `rebuild_affected_listener_managers` changes — the per-listener rebuild orchestration lives in one place (`routes/l4_rebuild.rs`), the clear-pass for active-but-no-routes listeners is required, and the gRPC / HTTP exclusion is deliberate.
knowledge_type: regression-guards
---

# L4 Rebuild Orchestration — Centralized in `l4_rebuild.rs`

**Audit date**: 2026-05-28
**Linked issue**: `path-norm-nonhttp-routes` F-007 (closed by extracting the
shared helper *and* baking in the missing clear-pass).

## Conclusion

All TCP / TLS / UDP `Global*RouteManagers::rebuild_all_listener_managers` and
`rebuild_affected_listener_managers` orchestration now flows through two
helpers:

- `pub(crate) fn rebuild_all_l4_listener_managers<R, M>(...)`
- `pub(crate) fn rebuild_affected_l4_listener_managers<R, M>(...)`

in `edgion-gateway/src/routes/l4_rebuild.rs`. The per-family `routes_mgr.rs`
methods are 13-line wrappers that fetch `pgis.all_listener_keys()` and
delegate, passing closures for `resolved_listeners_for_route`,
`get_or_create_listener_manager`, and `manager.rebuild`.

This sits alongside the sibling `apply_l4_partial_update` (in
`l4_partial_update.rs`) which already shared TCP/TLS/UDP `partial_update`
orchestration with the same `&'static str` log-tag + closure callback
convention.

## Required invariants — do not remove

1. **PGIS pre-create**: per-listener managers are created for every active
   listener (whether or not that listener currently has routes), so the
   listener-builder code always finds a stable `Arc<M>` to attach.

2. **Stale-sweep before clear-pass**: in `rebuild_all_l4_listener_managers`,
   the stale-sweep (drop `by_listener` entries with no routes AND not active
   in PGIS) **must** run before the clear-pass loop. Otherwise the clear-pass
   would emit a redundant empty snapshot on entries that are about to be
   removed.

3. **Clear-pass for active-but-no-routes** (the F-007 fix): after stale-sweep,
   walk `by_listener` once more and call `rebuild_one(entry, &empty_map)` on
   every entry whose key is **not** in `listener_buckets`. Without this pass
   a listener that previously held routes but has none after the rebuild
   keeps its old `ArcSwap<*RouteTable>` snapshot, and the data plane keeps
   forwarding to deleted backends. Removing the clear-pass to "save a pass"
   reintroduces F-007.

4. **PGIS access stays in wrappers, not in the helper**: each wrapper fetches
   `pgis.all_listener_keys()` once and passes it in as
   `active_listener_keys: Vec<ListenerKey>`. This is the seam that lets unit
   tests inject a synthetic active set without mutating the global
   `ListenerGatewayInfoStore` singleton (see the regression tests
   `test_rebuild_all_clears_active_listener_with_no_routes` in
   `tcp|tls|udp/routes_mgr.rs`). Inlining PGIS back into the helper would
   silently break test isolation.

5. **Removed managers are cleared before their last local `Arc` is dropped**:
   listener proxies retain their own `Arc<M>`, so deleting the registry entry
   alone does not retire the route table. Both the full-rebuild stale sweep and
   the partial-update orphan sweep must use the `Arc` returned by
   `by_listener.remove`, publish the empty table through `rebuild_one`, and only
   then drop it. The directly affected empty-bucket path already rebuilds the
   manager before removing it and must keep that order. TCP tests exercise the
   generic L4 helper with a genuinely non-empty retained handle; HTTP and gRPC
   have matching family-specific tests.

These invariants establish clearing, not Gateway delete/recreate recovery on a
retained proxy. PGIS is a logical-owner inventory; it does not prove a socket has
retired. See the [accepted restart boundary](../../listeners/intentional-design/gateway-recreation-requires-restart.md).

## Hard boundary — do not fold gRPC or HTTP into this helper

The same boundary as `apply_l4_partial_update`:

- **gRPC** (`grpc/routes_mgr.rs`): `route_cache` is `DashMap<String, Arc<GRPCRoute>>`,
  there is a separate `route_units_cache: Mutex<HashMap<..>>`,
  and rebuild goes via `store_route_table(build_grpc_route_rules_from_units(...))`,
  not `manager.rebuild(&HashMap)`. gRPC and HTTP already have the
  equivalent clear-pass.
- **HTTP** (`http/routes_mgr.rs`): its route-cache and domain-builder shape uses
  `build_domain_route_rules_from_routes` rather than the L4 unit API.

Any "let's also pull HTTP/gRPC into `l4_rebuild`" proposal is rejected for
the same reasons as the partial-update extraction (different per-listener
manager API surface, different rebuild input shape).

## Fix Suggestions Not Accepted

| Suggestion | Why not |
|---|---|
| Merge `rebuild_all` + `rebuild_affected` into one helper with `enum Mode { All, Affected(&HashSet) }` | They differ in four structural ways (PGIS pre-create, initial bucket population, stale-sweep predicate, orphan-sweep). Merging produces a function body the reader must re-bifurcate on every line. Keep the two-fn split. |
| Inline PGIS back into the helper | Breaks test isolation; reintroduces the global singleton dependency the wrapper-parameter design eliminated. |
| Drop the clear-pass; the pre-create loop already creates an empty manager | Pre-create only seeds new entries; entries that pre-existed from an earlier `rebuild_all` retain their `ArcSwap` snapshot from that earlier rebuild. Without the clear-pass, those snapshots survive — F-007 returns. |
| Add a per-family `clear` closure parameter instead of `rebuild_one(&empty)` | `TcpRouteTable::from_routes(empty) == ::new()`, same for TLS/UDP. Reusing `rebuild_one(&empty)` is equivalent and removes one parameter. |
| Fold the bucket-warn message back into "TCPRoute / TLSRoute / UDPRoute has no resolved_listeners…" per family | The `component=` structured field already discriminates. The generic message text is acceptable. |

## Re-evaluation Triggers

- New L4 family (QUIC, SCTP) lands → wire it up via two ~13-line wrappers.
- An L4 family adopts a different per-listener manager shape (owned routes,
  separate units cache, hostname/SNI second axis at the *global* layer) →
  reassess whether it still fits the helper or needs its own orchestration
  (mirror the gRPC carve-out).
- Profiling shows PGIS `all_listener_keys()` is hot enough that even the
  single snapshot per rebuild is a problem → consider caching at the wrapper.

## Reference cases

- F-007 fix commit (this entry's audit) — adds the helper and the clear-pass.
- Sibling pattern: [`l4-duplex-relay-centralization.md`](l4-duplex-relay-centralization.md)
  — the same shape applied to the bidirectional relay loop.
- Sibling pattern: `routes/l4_partial_update.rs::apply_l4_partial_update`
  — established the `&'static str` log-tag + closure callback convention
  this helper follows.
- HTTP canonical clear-pass (the model we mirrored):
  `edgion-gateway/src/routes/http/routes_mgr.rs:375-381`.
