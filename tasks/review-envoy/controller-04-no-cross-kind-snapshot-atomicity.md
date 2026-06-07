---
module: controller-translation
title: No cross-resource-kind snapshot atomicity — logically coupled changes arrive at the Gateway as separate incremental pushes
severity: medium
envoy_reference: Envoy's xDS protocol uses a snapshot model (LDS + RDS + CDS + EDS in a versioned snapshot) where all resource types are grouped into a single consistent version; a Gateway is never served a partial state where a Route references a Cluster that has not yet been pushed. Istio Pilot's PushContext is built as an atomic snapshot across all resource types before any Gateway is notified.
---

# No Cross-Resource-Kind Snapshot Atomicity for the Data-Plane Push

## Envoy/Istio design (the proven baseline)

Envoy's xDS and Istio's Pilot generate a _push snapshot_ that spans all relevant resource types (Listener, Route, Cluster, Endpoint) simultaneously. When a single user change (e.g., adding a backend Service) affects multiple xDS resource types, Pilot builds a new PushContext that includes all of the changes atomically before notifying any proxy. A proxy that receives a ListenerDiscoveryResponse is guaranteed that the RouteDiscoveryResponse and ClusterDiscoveryResponse describing the same route are already available in the same snapshot version.

The go-control-plane cache (`cache.SnapshotCache`) similarly stores a per-node snapshot of all resource types as a single versioned unit; updates to the snapshot are atomic across types.

## Edgion's current design

Edgion's `ConfigSyncServer` (`src/core/controller/conf_sync/conf_server/config_sync_server.rs`) maintains one `ServerCache<T>` per resource kind, each with its own independent `sync_version` counter (`utils::next_resource_version()` — a process-wide atomically-increasing integer).

When a single user action affects multiple resource kinds simultaneously (e.g., adding an HTTPRoute that references a new Service triggers: HTTPRoute processed → route pushed, then Service processed → Service pushed), the two kinds are pushed as separate incremental events with consecutive but independent `sync_version` values. There is no "this batch of changes is complete" barrier in the gRPC stream.

The `server_id` mechanism detects controller restarts but does not address the within-session cross-kind ordering gap. The Gateway client receives kind-specific Watch streams and processes them independently; there is no mechanism to hold a Route update until the corresponding Service update has also been received.

From `src/core/controller/conf_sync/cache_server/cache.rs:366-416`, each `apply_change()` call immediately notifies watchers via `notify.notify_waiters()` — there is no batching or cross-kind coordination:

```rust
ResourceChange::EventUpdate => {
    self.push_event(EventType::Update, resource, sync_version);
    // ← notify_waiters() fires here, immediately waking Gateway watchers
}
```

## The problem / gap

A Gateway data-plane proxy receiving incremental Watch events may observe a transient state where:

1. An HTTPRoute has been pushed (pointing to a backend Service or Secret that has been updated) but the updated Service/Secret has not yet been pushed.
2. A Gateway listener TLS certificate has been updated (Secret pushed) but the Gateway resource that references the new secret has not yet been pushed.

In practice this window is very short (milliseconds), but it is not zero. Under the following conditions the window can grow to seconds:
- The workqueue for different resource kinds runs independent workers; a backpressure event on one kind's worker does not block another kind.
- During init, each kind marks itself ready independently (`set_ready()` in `on_init_done()`); the Gateway can start syncing Kind A before Kind B has finished its init LIST.

The result is that the data-plane may briefly apply a logically inconsistent configuration — for example, a route pointing to a Service whose endpoint list has just been cleared, or a TLS listener using an old certificate for a short window after the spec change. While the Gateway is designed to tolerate missing references gracefully, the intermediate state is not intended to be visible to end-user traffic.

## Impact

- Brief traffic disruption during multi-resource updates (e.g., a Certificate rotation that requires updating both the Secret and the Gateway): there is a window where the Gateway has the new Secret but the old Gateway TLS config (or vice versa).
- The data-plane may attempt to establish TLS with a certificate it has already replaced or not yet received.
- No mechanism allows the Gateway to know "this set of changes forms one logical commit"; it cannot delay applying until all related kinds are received.
- This is a fundamental limitation of the current per-kind independent push design and is documented but not mitigated (unlike xDS, which solves it with a versioned multi-type snapshot).

## Suggested direction

Short-term: document the per-kind push ordering for operators, and ensure the Gateway's conf_handler is tolerant of transient partial state (check that missing referenced resources produce a safe default rather than a crash or permanent config error).

Medium-term: introduce a cross-kind "push epoch" concept — before notifying Gateway watchers, collect all pending changes across all kinds (draining a brief coalescing window, e.g., 5ms) and assign them a shared epoch tag. The Gateway client can then use the epoch to detect when a complete batch has arrived. This is directionally similar to how Istio's `PushContext` groups all changes for a single push cycle.

Long-term: consider aligning with xDS SotW (state-of-the-world) semantics, where the entire configuration state is resent atomically, eliminating partial-push concerns entirely at the cost of higher per-push volume.
