---
name: policy-store-remove-by-route-no-clone
description: Use when reviewing PolicyStore::remove_by_route per-entry clone-in-retain findings — remove_by_route now mutates route_refs in place via the &mut V retain provides; the pub(super)+contains pre-check suggestion is rejected.
---

# PolicyStore::remove_by_route Unconditional Clone in retain Closure

**Status / verdict**: fixed (lb-F008, 2026-05-24)

When reviewing findings of the form "`PolicyStore::remove_by_route` unconditionally clones every `ServiceLbPolicy` entry inside the `DashMap::retain` closure / O(N) wasted clones per route deletion / the clone runs under the shard write lock (lb-F008)": **fixed (lb-F008, 2026-05-24)**. `remove_by_route` (`edgion-gateway/src/lb/lb_policy/policy_store.rs`) now mutates `route_refs` in place through the `&mut V` that `retain` already provides — it calls the existing `remove_route_ref(&mut self, route_key) -> bool` directly on `policy` and returns `false`/`true` from its result, so there is no `policy.clone()` at all. On the delete path the in-place mutation is discarded with the dropped entry; on the keep path it is retained — behavior matches the old clone-then-write-back exactly. The issue's suggested fix (expose `route_refs` as `pub(super)` + a `contains` pre-check) is **not adopted**: the existing `&mut self` method already does the job without widening field visibility, and `HashSet::remove` on an absent key is a cheap allocation-free hashed lookup, so the pre-check buys nothing. Config-path only (route deletion), not the request hot path. Re-reports of the unconditional-clone form are not accepted; verify `remove_by_route` calls `remove_route_ref` on `&mut policy` (no clone) before re-reporting.
