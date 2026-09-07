---
name: listener-gateway-info-dual-arcswap-merged-snapshot
description: ListenerGatewayInfoStore previously kept two separate ArcSwap fields (`flat_data` and `index_data`) that were rebuilt by two consecutive `store()` calls, leaving a window where a reader could observe a cross-epoch pair. The store has since been refactored to a single `ArcSwap<ListenerGatewayInfoSnapshot>` that publishes both maps atomically. Reject re-reports that claim the rebuild is non-atomic.
knowledge_type: regression-guards
---

# runtime F-013 — `ListenerGatewayInfoStore` publishes flat + index via a single `ArcSwap` snapshot

**Status:** ✅ fixed · 2026-05-28
**Severity:** Low (BadLogic) — latent cross-epoch read pair

## Conclusion

`edgion-gateway/src/runtime/store/listener_gateway_info.rs` no longer carries two `ArcSwap`
fields. The struct is now:

```rust
struct ListenerGatewayInfoSnapshot {
    flat: HashMap<ListenerKey, Arc<Vec<GatewayInfo>>>,
    index: HashMap<ListenerKey, Arc<GatewayInfoIndex>>,
}

pub struct ListenerGatewayInfoStore {
    snapshot: ArcSwap<ListenerGatewayInfoSnapshot>,
}
```

`rebuild()` performs a single `self.snapshot.store(Arc::new(ListenerGatewayInfoSnapshot { flat, index }))`,
so any future caller that loads the snapshot once and reads both `flat` and `index` is
guaranteed to see a single-epoch pair. The public reader methods (`get`, `get_index`,
`all_listener_keys`, `stats`) keep their original signatures and now go through
`self.snapshot.load()`.

Re-reports of the form "rebuild stores flat_data then index_data non-atomically, a concurrent
reader can observe a cross-epoch pair" are **not accepted** — verify the current struct uses
the single `snapshot: ArcSwap<ListenerGatewayInfoSnapshot>` field before re-reporting.

## What was wrong (before the fix)

The previous shape was:

```rust
flat_data: ArcSwap<HashMap<ListenerKey, Arc<Vec<GatewayInfo>>>>,
index_data: ArcSwap<HashMap<ListenerKey, Arc<GatewayInfoIndex>>>,
```

`rebuild()` ended with two consecutive `store()` calls. Both maps are derived from the same
Gateway slice in the same rebuild call, but between the two stores a reader could observe
`flat_data` from the new epoch paired with `index_data` from the old epoch (or vice versa).

No caller in tree actually cross-reads both fields under a joint invariant today — the per-
request hot path in `pg_request_filter` uses only `get_index()`, the route managers'
rebuild/cleanup paths read only `flat_data` via `all_listener_keys()`, and `stats()` reads
only `flat_data`. The defect was purely latent, but the dual-`ArcSwap` shape was a
maintenance trap: a future caller assuming "every key in `flat_data` has a matching entry in
`index_data`" would silently fail during Gateway change events with no log or panic.

## Pattern note — jointly-invariant state must share one `ArcSwap`

When two pieces of shared state are derived from the same source and any consumer might
assume they are consistent (size, key set, cross-reference), they belong inside one struct
behind one `ArcSwap` (or one `RwLock`, etc.). Two separate `ArcSwap` fields swapped back to
back is *not* an atomic publish; it is a two-step sequence with a visible cross-epoch
intermediate state, regardless of how few cycles separate the two `store()` calls.

## Resolution references

- `runtime-F013` — 2026-05-28.
