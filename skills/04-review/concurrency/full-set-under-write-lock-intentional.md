---
name: full-set-under-write-lock-intentional
description: Use when reviewing `CacheData::reset()` in conf_sync cache_client; explains why `ConfHandler::full_set()` is dispatched while the `cache_data` write lock is held. Reject re-reports that the lock should be dropped before invoking the handler.
---

# `CacheData::reset()` dispatches `full_set` under the write lock — DESIGN-TRADEOFF

**Status:** ⚠️ accepted tradeoff · 2026-05-28
**Severity:** Medium (Perf — admin-API / readiness-probe latency spike during relist)

## Conclusion

`CacheData::reset()` (`edgion-gateway/src/conf_sync/cache_client/cache_data.rs:38-51`) replaces
`self.data`, calls `handler.processor.full_set(&self.data)`, and clears
`handler.compressed_events` — **all while the `parking_lot::RwLock<CacheData<T>>` write lock
is held** (taken by the two callers `cache.rs:106-112` and `event_dispatch.rs:117-119`). This
stays as-is.

Fix suggestions of the form "snapshot `self.data` under the write lock, drop the lock, then
call `full_set` on the snapshot" are **not accepted**.

## Core Rationale

**1. The harm is bounded transient on the control plane only.**

`full_set` only runs on a relist — initial startup (no readers yet), gRPC reconnect, or
server-switch. The blocked consumers are same-kind admin-API reads
(`list/get/keys/len/is_ready` on `ClientCache`) and readiness probes. The data-plane proxy
hot path reads from each manager's own `ArcSwap`/`DashMap` store (e.g. HTTPRoute's
`route_cache` + listener managers), not from `cache_data`, so request forwarding is
unaffected. The blocking window is exactly the duration of one `full_set` rebuild
(milliseconds to tens of milliseconds), and it happens at a moment when control-plane
disruption is already expected.

**2. The write lock is load-bearing for the `ConfHandler` contract.**

Every `ConfHandler<T>` implementation in the gateway (13+ of them across HTTPRoute,
TCPRoute, TLSRoute, gRPCRoute, UDPRoute, EdgionPlugins, EdgionStreamPlugins, EndpointSlice,
BackendTLSPolicy, Service, Gateway, EdgionTls, EdgionAcme, LinkSys, GatewayClass,
EdgionGatewayConfig, EdgionConfigData, ...) is written assuming `full_set` and
`partial_update` are **never invoked concurrently on the same handler**. The `cache_data`
write lock is what enforces that invariant:

- `reset` holds the write lock across the `full_set` call.
- The background `process_compressed_events` task (`cache_data.rs:128-218`) takes the same
  write lock before dispatching `partial_update`.
- `event_dispatch::apply_change` (`event_dispatch.rs:34-51`) takes the write lock to add
  a compress-event *and* update `self.data` atomically — the comment there explicitly says
  "Both compress_event and data update must be atomic under one lock so that
  process_compressed_events cannot observe an event whose data is not yet present".

Dropping the lock around `full_set` lets the post-debounce background task race the
in-flight rebuild. Concrete failure mode on HTTPRoute (`routes/http/conf_handler_impl.rs:54-77`):
`route_cache.clear()` + per-key insert + `rebuild_all_listener_managers()` interleaved with
a concurrent `partial_update`'s `route_cache.insert`/`remove` produces a transiently empty
or inconsistent listener manager view. Same shape applies to every other handler that
clears-then-refills.

**3. The "fix" expands scope to all 13+ handlers.**

To safely dispatch `full_set` outside the cache lock, one of the following is required:

- Add a separate dispatch mutex inside `CacheData` (e.g. `dispatch_lock: parking_lot::Mutex<()>`)
  that `reset` and `process_compressed_events` both take across the handler call — i.e. add
  a *new* lock to re-establish the same exclusion the existing lock already provides, for
  the sole purpose of releasing reads earlier. Net: one extra lock, identical handler-side
  semantics, gain is the read window during relist.
- Or push the exclusion responsibility into every `ConfHandler` implementation, which
  requires auditing and adding internal synchronization in all 13+ implementations.

Both options trade a bounded transient control-plane latency for a non-trivial concurrency
refactor and a new invariant for every future `ConfHandler` author to remember. Net: not
worth it.

## Fix Suggestions Not Accepted

- **"Populate `self.data` and `sync_version` under the write lock, drop the lock, then call
  `full_set` on a shallow-cloned `HashMap<String, Arc<T>>` snapshot."** — Removes the
  implicit `full_set` ⊥ `partial_update` exclusion that every handler relies on; introduces
  cross-call interleavings the handlers are not coded for. The shallow Arc-clone of the
  outer map is cheap, but cheapness of the clone is not the bottleneck.
- **"Apply the same release-before-dispatch shape to the `reset()` callers in `cache.rs`
  and `event_dispatch.rs`."** — Same root cause: shifts a load-bearing serialization
  boundary.

## Re-evaluation Triggers

Re-open this decision only if:

- Profiling shows the per-kind admin-API / readiness blocking during relist as a real
  operator-visible regression (not just theoretical), e.g. tens-of-seconds stalls instead
  of tens-of-milliseconds.
- A separate refactor moves `full_set` / `partial_update` serialization into the
  `ConfHandler` trait itself (handlers become self-synchronizing). At that point the
  cache-side write-lock-around-dispatch becomes redundant and removing it is a clean
  change.
- A new dispatch path is added that legitimately needs to interleave with `full_set`
  (currently none exists).

## Reference Cases

- conf-sync F-001 (review finding, `tasks/conf/`, 2026-05-28).
- [webhook-manager-rwlock-intentional.md](webhook-manager-rwlock-intentional.md) —
  sibling concurrency accepted-tradeoff: a lock kept intentionally because the
  finer-grained "fix" introduces more concurrency surface than the original cost.
- `edgion-gateway/src/conf_sync/cache_client/cache_data.rs` — `CacheData::reset`,
  `process_compressed_events`, `add_compress_event`.
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs` — `apply_change`,
  `list_and_reset`.
- `edgion-gateway/src/routes/http/conf_handler_impl.rs` — representative `full_set` /
  `partial_update` pair that assumes mutual exclusion.
