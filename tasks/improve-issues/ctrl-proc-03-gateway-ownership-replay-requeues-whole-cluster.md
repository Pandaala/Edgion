# ctrl-proc-03: Gateway ownership replay requeues every object of nine kinds cluster-wide, once per Gateway

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs:751-765`
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_ownership.rs:69-80,298-330`
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/gateway_route_index.rs` (`update_publication_ownership`, `clear`)
  - `edgion-controller/src/conf_mgr/runtime.rs` (`reset` clears `gateway_route_index`)

## Summary
In Kubernetes mode, whenever a Gateway's publication identity `(gateway UID, class UID)` changes,
`GatewayHandler::after_publish` requeues **every raw key** of HTTPRoute, GRPCRoute, TCPRoute, TLSRoute,
UDPRoute, EdgionTls, BackendTLSPolicy, EdgionBackendTrafficPolicy and EdgionAcme. These are all objects
in the watched scope, not the dependents of that Gateway. The stored identity starts as `None` and is wiped
by `ControllerRuntime::reset()`. Every Gateway therefore triggers a full-cluster replay on startup, on each
leader change or Controller restart, on Gateway recreation, and on GatewayClass recreation, which makes the
cost O(G × N).

## Current behavior (evidence)
- `gateway.rs:751-765`: when `update_publication_ownership` returns true, the handler runs
  `for kind in ownership_dependency_kinds("Gateway") { for key in processor.raw_keys() { ctx.requeue(kind, key) } }`.
- `ownership_dependency_kinds("Gateway")` lists 9 kinds (`status_ownership.rs:316-326`).
- `update_publication_ownership` (`gateway_route_index.rs`) returns true on any change of the
  `Option<(uid, class_uid)>`, including the first `None → Some` transition. `reset()` clears the index, so
  this repeats after every leadership/epoch reset.
- Each replayed Route reconcile requeues its parent Gateway unconditionally (ctrl-proc-01). That causes
  more Gateway epochs, which requeue EdgionTls/EdgionAcme again, and so on. Pending/scheduled dedup only
  collapses keys still waiting in the queue. Once a key has been processed, the next Gateway's replay
  enqueues it again.
- The comment justifies the full scan with "uncached ACME and transitive delegation references absent from
  the direct index". But `GatewayRouteIndex` already holds the direct Route/EdgionTls/EdgionAcme edges
  (`register_route_dependencies` runs before the Gateway read in TLSRoute/EdgionTls parse), and the
  delegation graphs hold transitive children.

## Industry comparison
controller-runtime maps a Gateway event to the Routes that reference it through a field indexer
(`IndexField(&HTTPRoute{}, "spec.parentRefs", …)` + `EnqueueRequestsFromMapFunc`). Istio's Gateway
controller recomputes status in one pass per push context instead of enqueuing every route once per gateway.
Neither replays the whole cluster for each parent.

## Impact
Startup and failover convergence time grows with G × N reconciles, plus the publish amplification from
ctrl-proc-01/02. With large route counts this collides with the 5000-entry workqueue channels
(ctrl-proc-04) and the `cache_ready_deadline_secs` restart loop documented in
`skills/02-features/02-config/00-controller-config.md`.

## Suggested direction
Replay only the dependents the index knows about. That means `gateway_route_index.get_routes_for_gateway(key)`,
the delegation-graph descendants of those routes, and the policy ancestors reachable via
`resolve_policy_ancestor_gateways_for_targets`. Add an explicit "pending ownership" reverse edge for
objects that parsed while their parent was not yet published, so that a missing edge is fixed where it
originates. If a global replay really is needed after a reset, perform it **once** after all Gateways
have published (as the post-init revalidation phase already does), not once per Gateway.

## Notes
- Checked `skills/04-review/status/regression-guards/canonical-parent-status-ownership.md`,
  `deployment-partitioned-observations.md` and `handler-requeue/*`. None of them accepts the per-Gateway full
  replay cost. The guards cover correctness of ownership, which the suggested direction keeps by replaying
  the exact dependents.
- Related: ctrl-proc-01 (Route → Gateway unconditional requeue), ctrl-proc-04 (queue capacity).

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `handlers/gateway.rs:751-765`: on `update_publication_ownership` true, loops `processor.raw_keys()` for every kind in `ownership_dependency_kinds("Gateway")`.
- `status_ownership.rs:316-326`: nine kinds listed for "Gateway".
- `gateway_route_index.rs:220-228`: returns true on any `Option<(uid, class_uid)>` change, including first publication after reset.
- No review page under `skills/04-review/status/` or `handler-requeue/` justifies the per-Gateway full replay.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `handlers/gateway.rs:761`: `for key in processor.raw_keys()` replay over `ownership_dependency_kinds("Gateway")` still present; `gateway.rs`, `status_ownership.rs`, `gateway_route_index.rs` unchanged since `feature-08-30`.
- No new `skills/04-review/status/` or `handler-requeue/` page justifies the per-Gateway full replay.
