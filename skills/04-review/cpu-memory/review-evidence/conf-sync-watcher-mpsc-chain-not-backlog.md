---
name: conf-sync-watcher-mpsc-chain-not-backlog
description: Distinguish batch counts from byte retention in the bounded conf_sync watcher queue chain.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# conf_sync Watcher Queue Backpressure and Payload Retention

## Current conclusion

The four configuration payload queues (typed Watch, JSON conversion, server-id
attachment and gRPC response) now have capacity one. This change accompanies
`waf-conf-sync-message-size-contract`, which supports larger compressed snapshots
and updates. The channels still await sends and propagate backpressure; a lagging
watcher can still recover through TooOldVersion and List.

Queue capacity is measured in batches. Multiplying batches by events per batch
and bytes per event is valid when estimating retained memory; the historical
claim below that this multiplication is a unit error is incorrect. Large WAF
resources invalidate its assumption that each event is only a few KiB. Count
bounds do not establish byte bounds, and a bounded ring can retain large resource
copies. No fixed process RSS guarantee follows from the payload codec budget.

Re-evaluate for measured backlog or memory pressure, increased batch/resource
sizes, or changed cancellation/backpressure behavior. See
[payload compression](../../conf-sync/regression-guards/bounded-payload-compression.md).

## Historical review evidence (superseded capacity and memory assumptions)

The following records the original argument, not an active rule suppressing
memory findings. Its backpressure mechanism remains relevant; its three-layer
count, multiplication objection, small-payload bound and blanket verdict do not.

**False-positive scenario**: the three `mpsc::channel(100)` in `conf_sync/cache_server/cache.rs:221,299` and `conf_sync/conf_server/grpc_server.rs:305` are counted as "per-watcher × 3 channels × 100 capacity × N events = 40000+ event backlog"; suggests reducing capacity or switching to a lock-free queue to alleviate memory.

**Reality**:

1. **The channel item is a batched `WatchResponse<T>`, not a single event**: at `cache.rs:140`, the watcher task uses `EventStore::get_events_from_sync_version` (`store.rs:187`) to bundle all unsent events from `from_version` into one `WatchResponse` and submit them. Channel capacity 100 = 100 *batches*; multiplying by event count is a unit error.

2. **All three mpsc layers submit via `.await`; backpressure is end-to-end**:
   - typed: `cache.rs:140` `sender.send(response).await`
   - simple: `cache.rs:321` `simple_tx.send(simple_response).await`
   - grpc outbound: `grpc_server.rs:331` `tx.send(Ok(response)).await`
   
   Downstream blocks → upstream blocks → watcher task stops pulling new events from EventStore. No `try_send`, no unbounded buffer, no event-loss bypass.

3. **EventStore is the source-of-truth and bounded**: `store.rs:153` is a circular queue (default capacity 200/kind; `mut_update` overwrites the oldest slot when full and advances `expire_sync_version`). When the watcher consumes slowly, events do not duplicate-pile-up in the channel — when the producer is stuck on `send().await`, new events overwrite the circular queue; next time the watcher wakes and finds `from_version < expire_sync_version`, triggering `WATCH_ERR_TOO_OLD_VERSION` (`store.rs:194`) so the client does a relist to realign. This is the designed fail-safe, not a memory leak.

4. **Steady state: channel close to empty**: `cache.rs:169` is `notify.notified()` driven; the watcher is woken once, pulls a batch, and goes back to wait. Only when a gRPC client peer's network is actually stuck or slow does the corresponding watcher's three-layer channel rise; this is itself a network failure symptom, not a memory issue determined by channel capacity.

5. **The three layers are decoupled with independent motivations, not stacked thoughtlessly**:
   - typed → simple: moves JSON serialization out of the store read lock (avoids long-held read locks)
   - simple → grpc: injects server reload detection (`grpc_server.rs:339` `check_interval` periodically checks server_id changes and proactively notifies the client to relist)
   
   Combining any two layers introduces new constraints (serializing inside the lock / mixing reload notifications into the watcher main loop). Capacity 100 at each layer is the empirical "leave a little buffer to absorb transient jitter" value; not a thoughtless large value being abused.

6. **Worst-case bound is controllable**: per-watcher all three layers full = 3 × 100 batches × ≤capacity events/batch × per-event a few KB ≈ tens of MB; requires that watcher's downstream connection to be completely stuck. N watchers all stuck simultaneously is a network/cluster-level failure scenario, not a channel capacity design defect; when it happens it is also auto-healed by `WATCH_ERR_TOO_OLD_VERSION` + relist.

**Verdict**: not a memory issue. Any "conf_sync watcher channel(100) chain causes backlog of N tens of thousands of events / should reduce capacity / should switch to lock-free structure" finding is closed per this entry, unless it is proved that (a) `try_send` or unbounded buffer bypasses backpressure, or (b) the EventStore circular queue's `WATCH_ERR_TOO_OLD_VERSION` relist path is broken, or (c) profile measurement shows that under normal network conditions the channel stays close to full at 100 for a long time.
