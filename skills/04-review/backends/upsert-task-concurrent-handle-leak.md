---
name: upsert-task-concurrent-handle-leak
description: HealthCheckManager::upsert_task now spawns then atomically swaps the per-service JoinHandle under a single write lock, aborting any displaced handle. Reject re-reports claiming concurrent reconcile_service calls for the same key leak orphaned health-check tasks.
---

# backends-F002 — Concurrent `upsert_task` no longer leaks orphaned health-check tasks

**Status:** ✅ fixed · 2026-05-21
**Severity:** High (BadLogic) — concurrent reconcile of the same service could orphan a spawned health-check task forever

## Conclusion

`HealthCheckManager::upsert_task` (`edgion-gateway/src/backends/health/check/manager.rs`)
now spawns the new task first, then performs a **single write-locked** swap:
`self.tasks.write().insert(service_key, handle)` returns the displaced handle (if any),
which is always `abort()`-ed. The write-locked `insert` is the linearization point, so the
last writer wins deterministically and every handle that leaves the map is aborted — there is
no longer a path where a freshly spawned handle is overwritten without being aborted.

Re-reports of the form "concurrent `upsert_task` / `reconcile_service` for the same
`service_key` orphans a tokio task", "the three-step cancel/spawn/insert sequence is
non-atomic", or "the second `insert` overwrites the first handle without aborting it" are
**not accepted** — verify the current `upsert_task` uses the spawn-then-atomic-swap pattern
before re-reporting.

## What was wrong (before the fix)

`upsert_task` did three non-atomic steps: (1) `cancel_task` took `tasks.write()`, removed and
aborted the old handle, then released the lock; (2) spawned a new task; (3) took `tasks.write()`
again to `insert`. The `tasks` field is a `parking_lot::RwLock<HashMap<…>>` with no per-key
serialization. Two concurrent `reconcile_service` calls for the same key — reachable because the
**Service handler** and **EndpointSlice handler** run on independent config-sync processor tasks
(`conf_sync/cache_client/cache_data.rs`, one `tokio::spawn` per resource type), with no shared
lock between them — could both pass `cancel_task` (the second finds nothing to abort), both spawn
a task, and the second `insert` would silently overwrite the first's handle. The orphaned first
task ran forever with no handle to cancel it, duplicating probe load and health-state writes
(a tokio task leak).

## Why the fix is correct

A single write-locked `insert` swaps the handle atomically and hands back whatever it displaced;
aborting that displaced handle guarantees every map entry that is replaced is also cancelled.
Two racing upserts each spawn their own handle outside the lock, then serialize on the `insert`:
whoever inserts last wins and aborts the other's handle, so no handle is ever leaked. `spawn`
and `abort` are non-blocking and run **outside** the held guard (no `await` under the lock), so
there is no deadlock or lock-across-await hazard. The no-runtime early-return branch still calls
`cancel_task`, preserving the original "even with no runtime, drop the existing task" semantics.

## Fix Suggestions Not Accepted

- "Introduce a per-service `Mutex`/`RwLock` to serialize `upsert_task` per key" — heavier than
  needed; requires a second keyed map plus its own lifecycle/cleanup. The single-lock atomic swap
  already eliminates the leak with no extra state.
- "Hold one write guard across `cancel_task` + `spawn` + `insert`" — works but holds the map-wide
  write lock across `spawn`; the swap-and-abort form keeps the critical section to just the
  `insert` and is equally leak-free.

## Re-evaluation Triggers

Re-open only if:

- `upsert_task` stops aborting the handle returned by `insert` (reintroducing the overwrite leak).
- The `tasks` map gains a code path that inserts a handle without going through the swap (e.g., a
  new method that bypasses the displaced-handle abort).

## Reference Cases

- backends-F002 (review finding, 2026-05-21)
- `conf_sync/cache_client/cache_data.rs` — one `tokio::spawn` processor task per resource type
  (Service vs EndpointSlice), the source of the cross-handler concurrency for the same service key.
