---
name: watch-cache-registry-get-or-create-slow-path
description: Use when reviewing findings that flag controller_id.to_string() twice in CenterWatchCacheRegistry::get_or_create write-lock branch as repeated stringify deserving Arc<str> or interning.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `CenterWatchCacheRegistry::get_or_create` Slow-Path `to_string()` is Not an Issue

**False-positive scenario**: `src/core/center/watch_cache/registry.rs:38-39` does `controller_id.to_string()` twice in the write-lock branch (once as HashMap key, once passed to `CenterWatchCache::new` for ownership); flagged as "repeated stringify; should switch to `Arc<str>` / string interning".

**Reality**:

1. **Slow path, once per controller lifetime**: the fast path of `get_or_create` is `caches.read()` + `Arc::clone`, with no `to_string()`. The two `to_string()`s only execute on first insert in the write-lock branch, corresponding to controller registration or disconnect-reconnect — a typical cluster has a few to a few dozen controllers, so total occurrences are a few dozen.
2. **Not on the request path**: the call source is `fed_sync::server` after receiving a `RegisterRequest`; it is called once, not per-event / per-request.
3. **Negligible absolute cost**: 2 small String allocations per call (controller_id is typically a few dozen bytes); far below the protobuf deserialization and SQLite upsert cost of a single gRPC registration request.
4. **`Arc<str>` does not save much**: at most 1 allocation can be saved (the HashMap key still needs to independently own the key type, unless `HashMap<String, _>` is also changed to `HashMap<Arc<str>, _>`, which is a larger cascading change). An intern pool is even less applicable — this is not the "stringify the same string thousands of times per second" scenario.

**Verdict**: not a performance issue. Any "controller_id `to_string()` in `get_or_create` / registry registration path should switch to `Arc<str>` / interning" finding is closed per this entry, unless it is proved that this path is reshaped to per-request triggering or a single registration involves ≥ thousands of controllers.
