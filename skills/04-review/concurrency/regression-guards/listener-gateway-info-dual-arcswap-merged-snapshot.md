---
name: listener-gateway-info-dual-arcswap-merged-snapshot
description: ListenerGatewayInfoStore publishes flat and indexed views through one immutable ArcSwap snapshot, sharing each GatewayInfo through Arc. Joint-epoch consistency requires one snapshot load, not independent reader calls.
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
    flat: HashMap<ListenerKey, Arc<[Arc<GatewayInfo>]>>,
    index: HashMap<ListenerKey, Arc<GatewayInfoIndex>>,
}

pub struct ListenerGatewayInfoStore {
    snapshot: ArcSwap<ListenerGatewayInfoSnapshot>,
}
```

`rebuild()` performs a single `self.snapshot.store(Arc::new(ListenerGatewayInfoSnapshot { flat, index }))`,
so any future caller that loads the snapshot once and reads both `flat` and `index` is
guaranteed to see a single-epoch pair. Each public reader method (`get`, `get_index`,
`all_listener_keys`, `stats`) loads the snapshot independently. Separate calls to
`get()` and `get_index()` can straddle a rebuild; they do not provide a joint-epoch
guarantee. The snapshot type is private, so public callers cannot currently request
both views through one load.

Each listener generation creates one `Arc<GatewayInfo>` shared by the flat view and
the indexed `ListenerEntry` values. `get()` returns `Arc<[Arc<GatewayInfo>]>`,
not the historical `Arc<Vec<GatewayInfo>>`. The original snapshot fix preserved
the public signatures at that time; that historical claim does not describe the
later per-entry Arc ownership change.

When reviewing atomicity, verify the current single snapshot publication and the
caller's load scope. The old two-store premise no longer applies, but independent
loads still require separate reasoning.

## What was wrong (before the fix)

The previous shape was:

```rust
flat_data: ArcSwap<HashMap<ListenerKey, Arc<Vec<GatewayInfo>>>>,
index_data: ArcSwap<HashMap<ListenerKey, Arc<GatewayInfoIndex>>>,
```

`rebuild()` ended with two consecutive `store()` calls. Both maps are derived from the same
Gateway slice in the same rebuild call, but between the two stores a reader could observe
`flat_data` from the new epoch paired with `index_data` from the old epoch (or vice versa).

At the original review, no caller cross-read both fields under a joint invariant;
the defect was classified as latent. The current per-request `pg_request_filter`
loads `get_index()`; `all_listener_keys()` and `stats()` each inspect the flat view
from one snapshot load. The dual-`ArcSwap` shape was a
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
- Current source tests: `flat_and_index_share_the_same_gateway_info`,
  `in_flight_gateway_info_survives_generation_replacement`, and
  `retained_gateway_identity_survives_store_replacement`. These check identity and
  retained-generation ownership, not joint consistency of independent loads.
