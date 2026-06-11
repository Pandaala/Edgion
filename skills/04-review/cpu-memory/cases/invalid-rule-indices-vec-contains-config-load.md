---
name: invalid-rule-indices-vec-contains-config-load
description: Use when reviewing findings that flag `invalid_rule_indices: Vec<usize>` linear-scan inside HTTP/gRPC route compile loops; cardinality is 0–1 in practice and the loop runs on the config-load path only, not per request.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `invalid_rule_indices` `Vec::contains` Linear Scan in HTTP/gRPC Route Compile Loops is Not an Issue

**Flagged scenario**: `append_route_to_hostname` (`src/core/gateway/routes/http/conf_handler_impl.rs`) and the sibling gRPC path (`src/core/gateway/routes/grpc/conf_handler_impl.rs`) iterate `validated.rules` / `rules.iter().enumerate()` and inside the loop call `invalid_rule_indices.is_some_and(|inv| inv.contains(&rule_id))` — a `Vec::contains` O(|invalid|) linear scan — for every rule (HTTP additionally per effective hostname). Net per-rebuild complexity for HTTP: O(H × R × |invalid|). The field is typed `Option<Vec<usize>>` end-to-end (`edgion-resources/src/resources/http_route.rs:59`, `edgion-resources/src/resources/grpc_route.rs:63`). Suggested fix: change the field type to `HashSet<usize>` or hoist a `HashSet<usize>` conversion outside the loop to make `contains` O(1); a `smallvec`-backed bitset would avoid heap allocation for the common single-element case.

**Reality**:

1. **Config-load path only, not per-request hot path**: both call sites run from `parse_http_routes_to_domain_rules` / the gRPC equivalent during `full_set` and `partial_update` rebuilds — i.e., on resource sync from Controller, not on the data-plane request/connection path. No per-request latency contribution.

2. **`|invalid|` is 0 or 1 in production**: `find_invalid_rule_indices` (`controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs` / `grpc_route.rs`) emits an index per rule that fails CORS / extension-ref / backend-ref validation; in any realistically operated cluster a route either has all rules valid (|invalid|=0) or one bad rule that the operator is in the middle of fixing (|invalid|=1). `Vec::contains` on length-0 returns instantly; on length-1 it is one integer compare — measurably faster than a HashSet probe (hash → bucket lookup → equality) on the same data.

3. **HashSet "optimization" can regress on the common case**: switching the field type to `HashSet<usize>` adds one heap allocation per route on every reload (vs. `Vec` with capacity 0/1 that the `kube-rs` deserializer can size exactly), and a per-rule `contains` against a 1-element HashSet does more work than the 1-element `Vec` scan it replaces. The call-site `HashSet::from_iter(...)` hoist variant adds an unconditional alloc per route even when `invalid_rule_indices` is `None` or empty — most routes pay a cost they did not pay before, in exchange for no measurable gain.

4. **Worst-case is operator-constructed, not adversarial**: the quadratic factor in H × R × |invalid| only becomes visible if a route is deliberately crafted with dozens of bad rules and dozens of hostnames. That state is itself a misconfiguration the operator will fix immediately because the listener is half-broken — not a steady-state to optimize for.

5. **Field type is part of the serialized resource shape**: `pub invalid_rule_indices: Option<Vec<usize>>` is `#[serde(default, skip_serializing_if = "Option::is_none")]` + `#[schemars(skip)]`, so it does not hit the CRD schema, but it is still serialized into the Controller-side resource representation and consumed by both `kube-rs` deserialization and any future federation / snapshot path. `Vec<usize>` round-trips losslessly; `HashSet<usize>` loses order and `serde_json` emits it as an array anyway. The type change is a non-feature.

**Verdict**: not a performance issue. Both the field-type change and the call-site `HashSet::from_iter` hoist are rejected. Re-open only if (a) a real production reload-stall report traces back to this loop, (b) `find_invalid_rule_indices` starts emitting per-rule diagnostic indices in quantities >10 per route (e.g. a new validation pass that fans out across every match clause), or (c) the call sites grow a second nested loop that turns the quadratic factor into a cubic one. Same class of decline as [status-diff-conditions-equal-on2-bounded.md](status-diff-conditions-equal-on2-bounded.md) — `Vec`-linear-scan on a bounded tiny-cardinality input is the right shape, HashMap/HashSet is the wrong shape.
