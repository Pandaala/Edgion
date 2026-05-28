---
name: conf-sync-F002-partial-update-under-lock-intentional
description: Use when reviewing `gateway/conf_sync/cache_client/cache_data.rs::process_compressed_events` "writes lock held across `partial_update` / CPU-bound listener rebuild blocks watch ingestion / parking_lot RwLock not yielding to Tokio" findings. Reject re-reports that propose snapshotting the inputs and dropping the lock before invoking `partial_update`.
---

# `CacheData` write lock kept across `partial_update` — DESIGN-TRADEOFF

**Status:** ⚠️ accepted tradeoff · 2026-05-28
**Severity:** Low (BadLogic / Concurrency — control-plane latency / Tokio worker stall)

## Conclusion

`CacheData::process_compressed_events` (`src/core/gateway/conf_sync/cache_client/cache_data.rs:128-218`)
keeps the `parking_lot::RwLock<CacheData<T>>` write guard from line 132 through the
`handler.processor.partial_update(add, update, remove)` call at line 202 and the final
`handler.compressed_events.clear()` at line 213. This stays as-is.

Fix suggestions of the form "convert `ConfHandlerData::processor` from `Box<dyn ConfHandler<T>>`
to `Arc<dyn ConfHandler<T>>`, snapshot the add/update/remove sets and clone the processor Arc
under the lock, drop the guard, then invoke `partial_update` with no lock held" are **not
accepted** under the current call-rate profile and per-resource-type lock topology.

## Core Rationale

**1. The lock is per-resource-type, and the data plane never reads it.**

Each `ClientCache<T>` owns its own `Arc<RwLock<CacheData<T>>>` (`cache.rs`), so an HTTPRoute
rebuild only blocks HTTPRoute watch ingestion and HTTPRoute admin reads — TLSRoute, GRPCRoute,
HTTPListener, EdgionPlugins, etc. are on independent locks. The request-routing path reads from
the handler's ArcSwap / DashMap stores (e.g. `routes_mgr.rs`'s `route_cache`, the ArcSwap-backed
`build_domain_route_rules_from_routes` output), never from this `CacheData` lock. There is no
data-plane outage; the cost is bounded to one type's control-plane operations.

**2. Real call rate is low and already debounced.**

`set_conf_processor` (`cache_data.rs:101-124`) spawns a Tokio task that waits on `Notify` and
then sleeps `BATCH_DEBOUNCE = 100ms` before each iteration, so multiple watch events within a
100 ms window collapse into a single `partial_update`. In steady state (no config churn) the
lock is never acquired at all; under bulk-apply churn one rebuild covers the whole batch.
`rebuild_affected_listener_managers` iterates the full `route_cache` once per batch (not once
per event) and only rebuilds the domain route table for the listeners affected by that batch,
so the lock-hold window grows with `route_cache` size, not with the number of events in the
batch. The per-type, per-batch cost the lock holds is what the snapshot-then-release refactor
would amortize — a refactor for an operation that fires `~0 times/sec` in steady state and
`~1 time/100ms` under churn.

**3. The "snapshot + drop guard" refactor has a real semantics cost the issue file glosses.**

Under the current single-lock posture, the watch path
(`event_dispatch.rs:332-351`) inserts data into `cache.data` **and** queues a `compress_event`
under one acquisition of `cache_data.write()`. `process_compressed_events` then reads both sides
under the same lock — the snapshot it builds is by construction consistent with the events it
acks. Dropping the guard before `partial_update` would mean (a) `processor` must be `Arc` so it
can outlive the guard, changing the basic shape of `ConfHandlerData`; (b) the `partial_update`
sees a snapshot that may have already been overwritten by a later watch event before the next
iteration runs, so any future "ConfHandler ⇄ CacheData" interaction now has to reason about
snapshot-vs-live skew that is currently impossible. Eventual consistency still holds (the next
notified iteration picks up the new events), but the "atomically observe what you ack" property
is gone. This is structural complexity charged against the maintainer in exchange for shaving a
control-plane stall that the per-type lock already isolates.

**4. The "Tokio worker stalls" framing overstates the cost on the default runtime.**

The default multi-thread runtime has `worker_threads = num_cpus`. The rebuild monopolizes one
worker for its duration; the others continue to drive data-plane traffic, other resource-type
watch loops, and admin handlers. There is no `block_in_place` / `spawn_blocking` regression here
because the work is in-memory CPU only (no I/O under the lock, as the issue itself confirms).
On a single-worker `current_thread` runtime the concern would be real — but the Gateway runs
the standard multi-thread runtime; the `current_thread` case is not a supported deployment.

## Fix Suggestions Not Accepted

- **"Convert `processor: Box<dyn ConfHandler<T>>` to `Arc<dyn ConfHandler<T>>`, snapshot the
  add/update/remove sets and the processor Arc under the lock, drop the guard, then call
  `partial_update` outside the lock"** — buys a per-type control-plane latency reduction that
  is non-measurable in steady state and bounded under churn, while introducing snapshot-vs-live
  skew that any future ConfHandler/CacheData interaction must reason about. Net new mental load
  for a near-zero observable win.

- **"Move `partial_update` onto `spawn_blocking` / a dedicated rayon pool"** — adds a thread
  hop on a path that runs ≤ 10× per second under bursty churn and is otherwise idle, and would
  require either a still-held lock to coordinate the spawn or the same snapshot+drop refactor
  above. Worse latency in the common path, no benefit in the rare path.

- **"Switch `CacheData`'s `RwLock` to `tokio::sync::RwLock` so the guard yields"** — async locks
  are slower per acquisition than `parking_lot::RwLock` and don't change the fact that the
  rebuild itself is CPU-bound; the holder still occupies its task for the rebuild duration.
  Net loss on the common case.

## Re-evaluation Triggers

Re-open this decision only if:

- Profiling under realistic bulk-apply churn (`>1000` HTTPRoutes applied within a `BATCH_DEBOUNCE`
  window) shows the rebuild stalling watch ingestion long enough to delay routing convergence
  past the admin-API timeout, OR shows the Tokio worker stall measurably starving data-plane
  traffic on the default multi-thread runtime.
- A new ConfHandler implementation lands that performs I/O (network, disk, IPC) under
  `partial_update` — at that point the "no I/O under the lock" property used in the rationale
  above stops holding, and the snapshot-then-release refactor becomes structurally justified.
- The Gateway adds a supported `current_thread` runtime deployment mode.
- `partial_update` becomes reachable from a hot per-request path (it is not today — the only
  caller is the `notify + sleep(100ms)` loop spawned in `set_conf_processor`).

## Reference Cases

- conf-sync-F002 (review finding, `tasks/conf/conf-sync-F002-partial-update-under-write-lock.md`,
  2026-05-28).
- [lock-around-atomic-metrics.md](lock-around-atomic-metrics.md) — sibling concurrency rule:
  the hot-path-vs-control-plane decision framework used to decide whether to split
  inside-lock / outside-lock work.
- [link-sys-F011-webhook-manager-rwlock-intentional.md](link-sys-F011-webhook-manager-rwlock-intentional.md)
  — sibling accepted-tradeoff: a coarse lock kept intentionally because the fine-grained "fix"
  is structurally heavier than the cold-path benefit.
- `src/core/gateway/conf_sync/cache_client/cache_data.rs` — `CacheData<T>`,
  `process_compressed_events`, `ConfHandlerData`.
- `src/core/gateway/conf_sync/cache_client/event_dispatch.rs:332-351` — the watch ingestion
  path that takes the same `cache_data.write()` for `insert` + `add_compress_event`.
- `src/core/gateway/routes/http/routes_mgr.rs::rebuild_affected_listener_managers` — the
  CPU-bound work whose duration sets the lock-hold window.
