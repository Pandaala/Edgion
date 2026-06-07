---
module: config-sync
title: Slow Gateway stalls the Controller watcher task via bounded-channel backpressure
severity: high
envoy_reference: xDS flow control — management plane must not be blocked by a slow subscriber; per-client isolation
---

# Slow Gateway stalls the Controller watcher task via bounded-channel backpressure

## Envoy's design (the proven baseline)

Envoy's management plane sends responses over a gRPC server-streaming RPC per
resource type per client.  The crucial design property is **per-client isolation**:
a slow or stuck client that cannot consume responses fast enough does *not* stall
the management plane's own event loop or affect other clients.  The management
plane tracks per-client state (last ACKed version) and can drop or replace
pending updates for a lagging client without blocking progress for healthy clients.
If a client's send buffer fills up, the management plane either drops the update
(relying on the next update to supersede it) or closes the stream, forcing the
client to reconnect and re-sync — but it never blocks the event processing path.

## Edgion's current design

On the Controller side, `ServerCache<T>::start_watcher_task`
(`src/core/controller/conf_sync/cache_server/cache.rs` lines 125-206) spawns one
`tokio` task per watch client.  That task holds a reference to the shared
`EventStore<T>` and sends events over a bounded `mpsc::channel(100)` to the next
stage.

The send is unconditional and awaited:

```
// cache.rs line 146
if sender.send(response).await.is_err() {
    break;
}
```

If the downstream channel stage (the JSON-serialization task in `watch_json`,
`cache.rs` lines 305-343) is slow, the `sender.send(...).await` blocks the
watcher task.  While blocked, this task holds no locks, but it also cannot
advance `from_version` or detect that the Gateway has dropped off.  The
chain continues: `watch_json` serializes and sends to another `mpsc::channel(100)`
in `ConfigSyncServer::watch` (`config_sync_server.rs` line 207), which in turn
feeds the gRPC stream task in `grpc_server.rs` (another `channel(100)`, line 247).

Four 100-slot channels are chained between `EventStore` and the gRPC wire:
`data_tx` (cache.rs:227) → `simple_tx` (cache.rs:305) → `tx` in
`ConfigSyncServer::watch` (config_sync_server.rs:207) → `tx` in
`ConfigSyncGrpcServer::watch` (grpc_server.rs:247).  If the gRPC send is blocked
(e.g., the Gateway is overloaded or the network is congested), all four stages
fill up and backpressure propagates upstream.  The watcher task is then stuck
waiting on a full channel while holding its `loop` iteration alive.

Additionally, when `notify_waiters()` is called in `ServerCache::push_event`
(`cache.rs` line 275), all watcher tasks wake simultaneously and attempt to read
from the `EventStore<T>` under an `RwLock`.  A slow client's watcher task will
wake, attempt `sender.send(...).await`, and yield back to the Tokio scheduler —
but the channel send will never complete until the downstream drains.  With
many connected Gateways this wastes waker slots and adds RwLock contention
on every `notify_waiters` burst.

## The problem / gap

A single slow or stuck Gateway can:

1. Block its own watcher task indefinitely (harmless in isolation), but the
   blocked task still participates in `notify_waiters()` wake-ups, repeatedly
   acquiring the store `RwLock` read lock, adding contention for all other
   watcher tasks.

2. Fill all three 100-slot intermediate channels.  Once full, the slow-path
   watcher task is permanently blocked in `send(...).await`.  The Controller can
   no longer deliver events to that Gateway even when the underlying TCP
   connection recovers, because the task is stuck at the channel send, not at
   the TCP send.  The only recovery path is for the Gateway to disconnect,
   which drops the channel receiver and unblocks the `send` (returning `Err`).
   HTTP/2 keepalive (10-second interval, 5-second timeout, `grpc_server.rs`
   lines 10-13) will eventually detect the dead connection, but there is a
   15-second window where the Gateway appears alive but receives no new events.

3. There is no timeout on the channel send.  A Gateway that is slow but still
   alive (keepalive succeeds) can hold the watcher task stuck for an arbitrary
   duration, preventing it from detecting `TooOldVersion` or `EventsLost`
   conditions that would normally trigger a relist.

## Impact

- During a traffic spike that slows a Gateway's event-processing loop, the
  Controller's watcher tasks for that Gateway fill up, the Gateway stops receiving
  incremental config updates, and silently falls behind.  It will not know it is
  behind (no NACK, no timeout) until either the stream drops or a new Watch
  reconnect happens.
- In a fleet where one Gateway is on a degraded node, its watcher tasks consume
  Tokio worker resources on the Controller, indirectly degrading scheduling
  latency for healthy Gateways' tasks.
- The 64 MB hard limit on List responses (`config_sync_server.rs` lines 148-170)
  protects against OOM on the List path but there is no equivalent protection on
  the Watch path: a large batch of events pushed into a full chain of channels
  will simply block.

## Verification note

Second-pass review confirmed: the unconditional `sender.send(response).await`
(cache.rs line 146) is real, no `send_timeout` or `try_send` exists anywhere in
the conf_sync controller path.  The original finding stated "three 100-slot
channels" — the actual count is **four** (corrected above).  The claim that a
blocked send occupies a Tokio *worker thread* is slightly imprecise: async await
yields the task to the scheduler rather than pinning an OS thread, but the
logical effect (task cannot advance, event delivery stalls) is the same.  All
core concerns are confirmed.  Severity remains **high**.

## Suggested direction

Replace the unconditional `send(...).await` in `start_watcher_task` with a
bounded `send_timeout` or `try_send` with a drop policy: if the downstream
channel is full after a short deadline, record a "events dropped for slow client"
metric, close the watcher task, and rely on the Gateway's reconnect logic to
trigger a relist.  This is the same philosophy as Envoy's management plane, which
treats a lagging client as one that must re-sync rather than one that must be
waited on.  Alternatively, use an unbounded channel per-client but cap the total
number of buffered events per client and drop the entire client if the cap is
exceeded, forcing a relist.  Either way, the key invariant is: a slow Gateway
must not block the Controller's event delivery path.
