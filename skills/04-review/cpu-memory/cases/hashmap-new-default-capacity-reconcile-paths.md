---
name: hashmap-new-default-capacity-reconcile-paths
description: Use when reviewing findings that suggest adding with_capacity() to HashMap::new() on full_set or reconcile paths to reduce rehashing; these are cold event-level paths with tiny data scale.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `HashMap::new()` Default Capacity on Reconcile / Full-Sync Paths

**False-positive scenario**: the audit flags Center `metadata_store::full_set` (`src/core/center/metadata_store/mod.rs:182-184`'s `new_cluster` / `new_service` / `new_gir`) and other `full_set` / reconcile paths' `HashMap::new()` for not using `with_capacity(data.len())`; suggests preallocation to reduce rehashing.

**Reality**:

1. **Cold path**: all `full_set` implementations live on Controller reload / Gateway config sync / Center fed_sync full push paths; trigger frequency = event level (Controller reconnect, reload, full re-push); **not request level**.
2. **Tiny scale**: in typical scenarios, `data.len()` is the count of a single Controller's PluginMetaData / single resource type, usually a few dozen entries (see the now-removed H6 review conclusion). Going from 0 to 64 capacity in HashMap involves ~3-4 rehashes; total cost O(64), microseconds level.
3. **Change surface vs benefit mismatch**: the project has 20+ `full_set` implementations and even more `HashMap::new()` on other reconcile / config-build paths. Bulk-changing to `with_capacity(data.len())` involves substantial signature/call-site changes; profile shows no benefit at all — same category as the already-judged "not an issue" "small-object operations on reconcile / config sync paths" (see the previous "Small-object clones on Resource reconcile / config sync paths are not issues").

**Verdict**: not a performance issue. Any "reconcile / full_set / config sync path `HashMap::new()` should use `with_capacity`" finding is closed per this entry, unless (a) the HashMap is allocated ≥ a thousand times per second on the request path, or (b) per-call `data.len()` is in the tens of thousands and rehash cost is observable.
