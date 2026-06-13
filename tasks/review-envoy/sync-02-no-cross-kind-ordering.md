---
module: config-sync
title: No cross-kind dependency ordering — routes can be applied before their backends arrive
severity: high
envoy_reference: xDS resource warming and CDS-before-EDS / LDS-before-RDS ordering guarantee
---

# No cross-kind dependency ordering

## Envoy's design (the proven baseline)

Envoy's xDS protocol defines an explicit resource warming sequence.  Clusters
(CDS) must be ACKed before the Listeners (LDS) that reference them are
considered active.  Within a cluster, EDS data must arrive before the cluster
enters the load-balancing pool.  The management plane enforces delivery order
(CDS → EDS → LDS → RDS) so that when a Listener activates, all resources it
depends on are already populated on the data plane.  A newly pushed Listener
stays "warming" — not serving traffic — until its RDS RouteConfiguration is
resolved.  This make-before-break guarantee prevents a transient window where
traffic arrives at a listener that has no valid route or upstream.

## Edgion's current design

Each resource kind is synced independently via a separate `Watch` RPC stream.
`start_watch_kinds` in `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs`
(lines 229-277) iterates over `effective_kinds` and calls `cache.start_watch_dyn()`
for each kind without any sequencing constraint.  Every `start_watch_dyn` spawns
its own independent `tokio::spawn` task (`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs` line 135), each of
which runs a List → Watch loop independently and calls `full_set` or
`partial_update` on its ConfHandler as soon as data arrives.

There is no coordination between, for example, the HTTPRoute cache task and the
Service/EndpointSlice cache tasks.  The Gateway startup readiness check
(`wait_for_ready` in `edgion-gateway/src/cli/mod.rs` lines 201-214) waits for all caches to
report `is_ready`, but that only means each cache has completed at least one
successful List call — it does not enforce any ordering within the startup
sequence, and it is irrelevant to runtime incremental updates.

## The problem / gap

**At startup**: The HTTPRoute ConfHandler (`edgion-gateway/src/routes/http/conf_handler_impl.rs`) calls
`full_set` which immediately rebuilds the listener match engines
(`rebuild_all_listener_managers`).  If the EndpointSlice cache task completes its
first List before the Service cache task does (or vice versa), the route manager
will start resolving backends against a partially populated store.  The
`preload_load_balancers` call in `edgion-gateway/src/cli/mod.rs` line 332 runs *after*
`wait_for_ready` returns, so by then all caches have data — but the individual
`full_set` callbacks fire *during* the List phase, before `wait_for_ready`
completes, meaning early route rebuilds can observe an incomplete world.

**At runtime (incremental updates)**: When an HTTPRoute update arrives that
references a new Service/EndpointSlice, the route handler's `partial_update`
fires immediately.  The new backend may not yet exist in the Service or
EndpointSlice caches because the corresponding Watch events for those kinds have
not yet been processed by their independent tasks.  The route will be installed
referencing a missing upstream.

**New Gateway/Listener creation**: A new Gateway resource (triggering listener
creation) and the EdgionTls resource (TLS cert) it depends on are synced by
independent tasks.  If the Gateway Watch event fires before the EdgionTls event
is processed, the listener may start without the TLS cert being available yet.

## Impact

- During a rolling deploy or Controller restart, a Gateway instance may briefly
  serve 502/503 errors because an HTTPRoute references a Service that does not
  yet have EndpointSlice entries in the local cache.
- On initial Gateway startup with a large config, the order in which kind-specific
  Watch tasks receive their first batch is non-deterministic.  A route that
  references a not-yet-populated upstream will resolve to zero backends and
  Pingora's load balancer will return errors for those routes until the backend
  cache is populated (typically within seconds, but the window is unbounded under
  load).
- Adding a new HTTPS listener that references a TLS cert may briefly start
  without the cert if the EdgionTls cache task has not finished its first List.

## Suggested direction

Define a dependency graph between resource kinds (e.g., HTTPRoute depends on
Service and EndpointSlice; Gateway depends on EdgionTls; etc.).  During the
initial sync, hold off calling `full_set` on higher-level kinds (routes, gateways)
until all their dependency kinds have completed their first successful List.  A
simple approach is a startup barrier: once all caches report ready, do a single
coordinated full-set rebuild in dependency order rather than firing individual
callbacks during the List phase.  For runtime incremental updates, establish a
read dependency: the route handler's `partial_update` should check the Service
and EndpointSlice caches for the referenced backends and mark any unresolved
references as "pending" rather than installing an empty-upstream route.
