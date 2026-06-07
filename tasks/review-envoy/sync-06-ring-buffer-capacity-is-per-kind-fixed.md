---
module: config-sync
title: Fixed-capacity per-kind ring buffer — high-churn kinds silently lose events, forcing expensive full resyncs
severity: low
envoy_reference: xDS delta (incremental) resource protocol — explicit per-resource add/remove with no ring-buffer expiry
---

# Fixed-capacity per-kind ring buffer — high-churn kinds silently lose events, forcing expensive full resyncs

## Envoy's design (the proven baseline)

Envoy's Delta xDS (incremental xDS) protocol sends explicit per-resource
additions and removals.  There is no ring buffer on the management plane: every
resource change is sent to every subscribed client exactly once (or the stream is
torn down and the client re-syncs).  The management plane does not expire events.
If a client reconnects it sends its known resource versions and the management
plane sends only the diff.  This design avoids the "TooOldVersion" problem
entirely: there is no sliding window of retained events that can be outpaced by
a fast-churning resource type.

## Edgion's current design

The Controller-side `EventStore<T>` is a fixed-capacity circular ring buffer
(`cache_server/store.rs` lines 157-189).  The capacity is set at construction
time and wraps around, evicting the oldest events when full.  When an event is
overwritten, `expire_sync_version` is advanced to the overwritten event's
`sync_version`:

```rust
// store.rs lines 180-185
if self.end_index - self.start_index >= self.capacity {
    if let Some(last_event) = self.cache[index].as_ref() {
        self.expire_sync_version = last_event.sync_version;
    }
    self.start_index += 1;
}
```

A Gateway whose `from_version` falls below `expire_sync_version` receives a
`WATCH_ERR_TOO_OLD_VERSION` error (`store.rs` lines 199-201), which causes the
Gateway to break out of the watch loop and perform a full relist.

The default capacity appears to be 200 events per kind (`EventStore::new` in
`store.rs` line 159 uses the provided capacity or 200 as default; the
`ServerCache::new` call signature shows `capacity: u32`).  For high-churn kinds
(EndpointSlice updates during a rolling deploy; HTTPRoute changes during a policy
rollout), 200 events can be exhausted in seconds.

## The problem / gap

1. **Capacity is per-kind tunable via config, but defaults are uniform and
   metrics are missing**: `ConfSyncConfig` (src/core/common/config/mod.rs
   lines 324-370) exposes `default_capacity` (global default, defaults to 200)
   and `capacity_overrides` (per-kind override map).  So operators can tune
   capacity per kind without recompiling.  However, the defaults are uniform —
   EndpointSlice and GatewayClass both start at 200 unless explicitly overridden —
   and there is no documentation guiding operators to increase EndpointSlice
   capacity for large clusters.  The original claim that "there is no per-kind
   capacity setting" is incorrect; the claim that the defaults are inadequate
   for high-churn kinds remains valid.

2. **TooOldVersion triggers a full List, which is expensive**: a full List for
   EndpointSlice can be large (many pods × many services).  The 64 MB hard limit
   on List responses (`config_sync_server.rs` lines 148-170) exists precisely
   because this can be large.  A frequent TooOldVersion on EndpointSlice means
   repeated large List operations, which add CPU (serialization) and network load
   to the Controller, and increase the time during which the Gateway is running
   stale backend data.

3. **A slow Gateway (due to `sync-03` slow-client stall) is especially
   vulnerable**: if a Gateway's watcher task is blocked on a full channel (the
   slow-client scenario), events accumulate in the EventStore, wrap around, and
   expire.  When the Gateway finally reconnects and tries to resume from its last
   `from_version`, it will almost always receive `TooOldVersion` and be forced
   into a full relist.  This creates a feedback loop: slow client → TooOldVersion
   → full relist (larger data volume) → slower client.

4. **No visibility into ring-buffer utilization**: there are no metrics tracking
   `expire_sync_version` advancement, how close the ring buffer is to wrapping,
   or how many `TooOldVersion` errors have been returned per kind.  Operators
   cannot proactively detect that a kind's ring buffer is too small before clients
   start missing events.

## Impact

- During a Kubernetes rolling update that churns hundreds of EndpointSlice events,
  all connected Gateways may receive `TooOldVersion` and simultaneously issue full
  List requests for EndpointSlice.  This is a thundering-herd problem that
  significantly increases Controller load at exactly the moment when the cluster
  is already under pressure (rolling update).
- A Gateway that was temporarily disconnected (network partition, pod restart)
  will almost certainly receive `TooOldVersion` on reconnect for any high-churn
  kind, forcing a full relist instead of the intended incremental resume.
- The 200-event default is likely sufficient for slow-moving kinds (Gateway,
  HTTPRoute) but insufficient for EndpointSlice or Service in large clusters.

## Verification note

Second-pass review: the ring buffer mechanics, TooOldVersion trigger, and
feedback-loop concerns are all verified as real.  The original finding's first
sub-point was wrong — per-kind capacity IS configurable via
`conf_sync.capacity_overrides` in the controller config (corrected above).
The missing pieces are: (a) the uniform 200-event default is not documented as
insufficient for high-churn kinds; (b) no metric exists for ring-buffer
utilization or TooOldVersion rate.  Severity downgraded from **medium** to
**low**: the fundamental gap (TooOldVersion forcing expensive relists) is real
but operators have the tool to address it; the main issue is lack of guidance
and observability.

## Suggested direction

Short-term: document recommended `capacity_overrides` values for high-churn
kinds (EndpointSlice, Service) in large clusters.  Add metrics for
`expire_sync_version` advancement rate and `TooOldVersion` error rate per kind.

Medium-term: consider moving to an event-compaction model for high-churn kinds:
instead of a ring buffer of raw events, maintain a compacted "current state" per
resource key plus a small change journal.  A reconnecting Gateway that has missed
many events receives the current full state for the resources it was watching,
rather than being forced to do a full List of all resources for that kind.  This
is closer to the Delta xDS model.
