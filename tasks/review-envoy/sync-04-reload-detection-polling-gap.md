---
module: config-sync
title: Controller-reload detection relies on 1-second polling, not on a protocol signal
severity: medium
envoy_reference: xDS management-plane restart handling — stream teardown forces immediate reconnect; no polling gap
---

# Controller-reload detection relies on 1-second polling, not on a protocol signal

## Envoy's design (the proven baseline)

In Envoy xDS, when the management plane restarts or loses state, the stream is
torn down from the server side.  The gRPC connection itself breaks, forcing the
client to reconnect immediately (subject to backoff).  Upon reconnect the client
sends a fresh DiscoveryRequest advertising its current resource versions.  The
management plane can then respond with only what changed.  Critically, the
detection latency is bounded by the gRPC keepalive timeout — there is no
additional polling loop layered on top.  The protocol itself is the notification
mechanism.

## Edgion's current design

When the Controller reloads (reconnects to the K8s API server or performs a
manual relink), it regenerates a `server_id` (`config_sync_server.rs` lines
95-105) and needs to notify connected Gateways.  The notification mechanism is a
**1-second polling loop** inside the gRPC Watch stream task:

```rust
// grpc_server.rs lines 258-313
let mut check_interval = tokio::time::interval(std::time::Duration::from_secs(1));
loop {
    tokio::select! {
        event = receiver.recv() => { ... }
        _ = check_interval.tick() => {
            if let Some(new_server) = provider.config_sync_server() {
                let new_server_id = new_server.server_id();
                if new_server_id != initial_server_id {
                    // send WATCH_ERR_SERVER_RELOAD and break
                }
            }
        }
        _ = shutdown_token.cancelled() => { break; }
    }
}
```

The same polling pattern is duplicated in `watch_server_meta`
(`grpc_server.rs` lines 424-449).

After the Controller's `EventStore` is cleared and `server_id` is regenerated,
existing Watch streams continue to run for up to one full second before the
polling tick fires and sends the `WATCH_ERR_SERVER_RELOAD` signal.  During this
window:

- On a Controller reload, a brand-new `ConfigSyncServer` instance is created
  (`ConfigSyncServer::new()`, `kubernetes/center.rs` line 822 or
  `non_kube_common.rs` line 205); the `provider.config_sync_server()` call in
  the polling tick (`grpc_server.rs` lines 290-293) detects the instance change
  by comparing `server_id` strings.  Note: `reset_for_relink` in
  `config_sync_server.rs` lines 298-308 exists as a utility method but is not
  called in the live reload path — the actual mechanism is new-instance creation.
- Watcher tasks for connected clients may also detect `EventsLost`
  (version jumped with no events, `cache.rs` lines 155-171) and send an
  `EventsLost` error to the client, causing the client to relist against the
  *new but partially-populated* Controller cache.
- The `WATCH_ERR_SERVER_RELOAD` signal sent by the polling tick is therefore a
  *redundant* notification that can arrive up to 1 second after the Gateway has
  already started relisting against the new store.

More concretely, the new `ConfigSyncServer`'s `EventStore` starts empty.  If a
watcher task for a still-connected Gateway has not yet seen the polling tick
that fires `ServerReload`, it may get `EventsLost` from the new (empty) store
(`notify_waiters()` in `ServerCache::clear`, `cache.rs` lines 68-73), triggering
a redundant relist.  The 1-second polling exists as a belt-and-suspenders
mechanism but introduces a detection gap and unnecessary complexity.

## The problem / gap

1. **Up-to-1-second gap between state change and notification**: during a busy
   Controller reload, existing Watch streams continue running for up to one
   second before the polling tick detects the new `ConfigSyncServer` instance
   and sends `WATCH_ERR_SERVER_RELOAD`.  During this window the Gateway may
   receive `EventsLost` (event-driven, faster path) from the new empty store,
   causing a relist that races with the subsequent `ServerReload` signal.

2. **Duplicated detection paths with different latencies**: `EventsLost` (fast,
   event-driven) and `ServerReload` (slow, 1-second polling) both attempt to do
   the same job.  The interaction between the two paths is not explicitly
   specified, and whichever fires first wins.  If `ServerReload` fires first it
   triggers a full relist; if `EventsLost` fires first it also triggers a full
   relist — but the semantics differ slightly (one resets `from_version` cleanly
   via a new List, the other sends an error that causes the Watch loop to break
   and relist).

3. **Polling overhead at scale**: with N Gateways each holding K kind-specific
   Watch streams, the Controller is running N*K polling loops that tick every
   second.  This is a constant CPU overhead even when no reload is occurring.

## Impact

- Under a quick Controller reload cycle (e.g., rapid K8s API server reconnect),
  a Gateway may receive two independent relist triggers within the same second:
  one from `EventsLost` and one from `ServerReload`.  The second relist is
  wasteful and increases the time to convergence.
- The 1-second detection gap means that after a Controller reload, Gateways will
  serve traffic based on a stale (cleared) config for up to 1 second before
  being told to relist.  For most resource kinds this means stale-but-not-broken
  serving; but for resources that were actively deleted in the reload-triggering
  event, the Gateway may continue routing to removed backends.
- Because reload creates a *new* `ConfigSyncServer` instance with a fresh empty
  store rather than mutating the existing one, the non-atomic clear + regenerate
  concern from the original finding does not apply.  However, the gap between
  when the new instance becomes active and when connected Gateways are notified
  still exists (up to 1 second), and the `EventsLost` vs `ServerReload` race is
  still real.

## Verification note

Second-pass review confirmed: the 1-second polling interval (`grpc_server.rs`
line 258) and the `EventsLost`/`ServerReload` dual-path race are real.  The
original finding incorrectly cited `reset_for_relink` as the reload mechanism;
the actual live-path is new-instance creation (`ConfigSyncServer::new()` in
`kubernetes/center.rs` line 822 / `non_kube_common.rs` line 205).
`reset_for_relink` is dead code in the current codebase.  All other claims
(polling overhead, N*K polling loops at scale, gap between store-clear and
Gateway notification) are verified as correct.  Severity remains **medium** —
the practical impact is a transient double-relist rather than a persistent
outage, but the polling complexity and O(N*K) tick overhead are real costs.

## Suggested direction

Replace the 1-second polling with a direct protocol-level notification.  When
the Controller decides to reload, it should send a terminal `WATCH_ERR_SERVER_RELOAD`
message on all active Watch streams *before* clearing the EventStore and
regenerating the server_id.  This makes the notification event-driven and
eliminates the race between `EventsLost` and `ServerReload`.  The `clear` and
`regenerate_server_id` operations should be made logically atomic with respect
to the notification (e.g., regenerate the server_id first, send the reload signal
to all active streams, then clear the EventStore).  The polling mechanism can
then be removed entirely, reducing constant CPU overhead and simplifying the
stream task logic.
