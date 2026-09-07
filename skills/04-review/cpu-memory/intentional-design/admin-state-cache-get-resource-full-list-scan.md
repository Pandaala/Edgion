---
name: admin-state-cache-get-resource-full-list-scan
description: Use when reviewing findings that flag cache_get_resource full-list plus linear-scan as O(N) alloc waste needing a get_by_key path; this is an admin/diagnostic path bounded by manual operator calls.
knowledge_type: intentional-design
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `AdminState::cache_get_resource` Full-List + Linear-Scan is Not a Performance Issue

**False-positive scenario**: `cache_get_resource` / `cache_get_cluster_resource` in `edgion-controller/src/api/types.rs` delegate to `cache_list_resources(kind)` (full serialize + parse of the whole `ServerCache<T>` snapshot to `Vec<serde_json::Value>`) and then linearly scan to find one entry; flagged as "O(N) allocations per single-resource lookup, should expose a `get_by_key` path through `WatchObj` / `ConfigSyncServer`".

**Reality**:

1. **Cold admin path, not data plane**: the only callers are `configserver_handlers.rs::get_resource` (the `GET /configserver/{kind}?namespace=…&name=…` admin endpoint) and the `edgion-cli --target server` debug flow. No per-request, per-connection, or per-reconcile traffic touches this code. Trigger frequency is manual ops/diagnostics — bounded by human keypresses.

2. **Response size is hard-capped**: `ConfigSyncServer::list` already enforces `MAX_LIST_BYTES = 64 MB` (`config_sync_server.rs:149`) and returns an error if the serialized kind exceeds that. So worst-case allocation per call is bounded; there is no unbounded-N risk.

3. **The "proper fix" expands a cross-cutting trait surface for no measurable benefit**: `ServerCache<T>::get_by_key` (`cache.rs:99`) exists and is O(1), but it is intentionally not on the `WatchObj` trait (`conf_server/traits.rs`). Adding `get_json` to `WatchObj` would require implementing it on every cache type and plumbing through `ConfigSyncServer::get`, all to save microseconds on a path nobody hot-loops. The trait deliberately exposes only `list_json` + `watch_json` because that's all the gRPC sync protocol needs.

**Decision criteria during review**:
- "`cache_get_resource` does full list + linear scan, should use `get_by_key`" → check the call site: is it admin/CLI? If yes, this entry applies.
- The two functions carry doc comments (since P3-c3b-006) explicitly stating the O(N) cost is intentional on this admin/diagnostic path.

**Verdict**: not a performance issue. Tracked as P3-c3b-006 with `confirmed-downgraded` disposition; closed with explanatory doc comments. Trait-surface-expansion fix suggestions are not accepted unless paired with profile evidence that this admin path dominates Controller CPU.
