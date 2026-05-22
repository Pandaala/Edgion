---
name: admin-api-controller-summaries-multi-call
description: Use when reviewing findings that flag controller_summaries() being called multiple times with partial field use as repeated full-field clone CPU waste; all callers are admin/ops paths at human scale.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Admin API `controller_summaries()` Multi-Call / Partial Field Discard is Not an Issue

**False-positive scenario**: the audit flags `ResourceAggregator::controller_summaries()` (`src/core/center/aggregator/mod.rs:123-135`) called in multiple Admin API handlers, with some call sites only taking one or two fields and discarding the rest; judged as "repeated full-field clone, CPU waste". Typical examples:

| Call site | Fields actually used | "Discarded" fields |
|-------|------------|-----------|
| `api/mod.rs:178` `list_controllers` | All 5 fields | None |
| `api/mod.rs:183` `list_clusters` | Only `.cluster` | controller_id / env / tag / online |
| `global_connection_ip_restriction_handlers.rs:53,161` `online_controllers` | filter `.online` + take `.controller_id` | cluster / env / tag |
| `region_route_handlers.rs:150,250,312` | Partial by controller_id / online | Same as above |

**Reality**:

1. **All are Admin API, not request path**: call sources are operations dashboard / CLI (list controllers / clusters / region routes / global IP restrictions); call frequency = human/operator scale, not data-plane request level.
2. **Limited scale**: per-Center, controller count typically a few dozen to a few hundred (far less than thousands). `ControllerSummary` only contains 2 short Strings + 2 normally-empty or very short `Vec<String>` + 1 bool; a single `collect` is at most a few KB.
3. **Multiple calls do not block each other**: `controller_summaries` internally is `RwLock::read()`; read locks are not exclusive; multi-handler concurrent calls have no lock contention.
4. **To "save" requires API-shape change**: to eliminate full-field clone, dedicated methods like `cluster_names()` / `online_controller_ids()` must be added to `ResourceAggregator` (API surface bloat + multiple repeated implementations), or change to `with_summaries(F)` callback (all call sites changed + read-lock-hold time controlled by callback, instead increasing concurrency risk). Profile shows no benefit at all.

**Verdict**: not a performance issue. Any "`controller_summaries` multi-call / partial field discard should use dedicated method or callback" finding is closed per this entry, unless it is proved that (a) the method is reshaped to be triggered per second on the request path, or (b) controller count is ≥ tens of thousands.
