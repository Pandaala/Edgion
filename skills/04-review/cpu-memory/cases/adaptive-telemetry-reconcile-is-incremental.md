# Adaptive telemetry reconciliation is route-contribution incremental

## Decision

Adaptive telemetry remains keyed by terminal HTTPRoute UID, terminal rule
identity, typed `EdgionBackend` target, and logical model. Do not move this
state into the AI-service store: doing so would either merge unrelated route
rules or let a delete-and-recreate route inherit the prior generation's
telemetry.

The Gateway maintains a cold-path contribution index instead:

- each HTTPRoute key owns parsed adaptive rule facts and its current telemetry
  key contribution;
- reverse indexes map EdgionPlugins keys and typed AI-service targets back to
  the route keys that reference them;
- route partial updates parse and recompute only changed route keys;
- plugin and AI-service partial updates recompute only reverse-indexed routes;
- a full HTTPRoute or EdgionPlugins snapshot may rebuild its complete indexed
  scope.

Publishing still reconciles the union of actual telemetry keys so equivalent
keys retain their state `Arc`, removed keys lose discoverability, and request
plans that already captured an `Arc` remain valid.

## Why the Gateway index is required

The Controller already has typed dependency managers and requeues routes when
referenced plugins or AI services change. That is necessary but not sufficient
for Gateway state: conf_sync streams for different resource kinds can arrive in
either order.

The Gateway index therefore retains reverse references even while a plugin or
service is missing. A route-first sequence initially contributes no telemetry,
then self-heals when the dependency arrives. A dependency-first sequence also
works because the later route update reads the current plugin and service
snapshots.

## Concurrency and hot-path boundary

Route matching tables are rebuilt and published while holding the route rebuild
lock. Adaptive contribution work runs after that lock is released. A separate
cold-path mutex serializes contribution mutations across route, plugin, and AI
service handlers; request selection continues to use only the immutable
`AdaptiveTelemetryStore` snapshot and never takes this mutex.

Do not replace this design with request-time `get_or_create`: that would add a
global map mutation to the data-plane hot path and weaken deterministic residue
cleanup.

## Required coverage

- Route-before-dependency ordering must self-heal without another route event.
- Updating one route must preserve unrelated route contributions and remove the
  old route UID contribution.
- Removing one of two source routes that contribute the same terminal identity
  must retain the shared key until the last source is removed.
- AI-service addition and removal must both be included in the changed-target
  set; additions are needed to wake routes that arrived first.
- Existing destructive-set coverage for route shape, plugin eligibility,
  namespace resolution, target type, and model catalog remains mandatory.
