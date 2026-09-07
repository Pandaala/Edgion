---
name: policy-store-remove-by-route-no-clone
description: Use when reviewing PolicyStore::remove_by_route per-entry clone-in-retain findings — remove_by_route now mutates route_refs in place via the &mut V retain provides; the pub(super)+contains pre-check suggestion is rejected.
knowledge_type: superseded
---

# PolicyStore::remove_by_route Unconditional Clone in retain Closure

**Status / verdict**: fixed (lb-F008, 2026-05-24); **obsolete (2026-07-10): the entire route-fed `PolicyStore` module was deleted** when LB policy moved off the route onto `EdgionBackendTrafficPolicy` (see `skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`).

Historical record, kept for context: when reviewing findings of the form "`PolicyStore::remove_by_route` unconditionally clones every `ServiceLbPolicy` entry inside the `DashMap::retain` closure / O(N) wasted clones per route deletion / the clone runs under the shard write lock (lb-F008)", the fix (lb-F008, 2026-05-24) made `remove_by_route` (formerly `edgion-gateway`'s `lb/lb_policy/policy_store.rs`, module deleted 2026-07-10) mutate `route_refs` in place through the `&mut V` that `retain` already provides — it called the existing `remove_route_ref(&mut self, route_key) -> bool` directly on `policy` and returned `false`/`true` from its result, so there was no `policy.clone()` at all. The issue's suggested fix (expose `route_refs` as `pub(super)` + a `contains` pre-check) was **not adopted**. There is no `PolicyStore`, `remove_by_route`, or route ref-counting anymore — LB policy resolution today is a single `DashMap<service_key, ParsedLBPolicy>` read owned by `BackendTrafficPolicyStore` (`edgion-gateway/src/backends/policy/backend_traffic/store.rs`), with no per-route bookkeeping to clone or mutate. Do not re-file against the old model.

## Resolution references

- `lb-F008` — 2026-05-24.
