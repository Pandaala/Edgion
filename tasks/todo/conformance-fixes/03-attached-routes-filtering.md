# AttachedRoutes Filtering

## Priority: High | Effort: Small | Tests Fixed: 4

## Affected Tests
- GatewayWithAttachedRoutes (subtest 2: expected 2, got 3)
- HTTPRouteHostnameIntersection (AttachedRoutes subtest: expected 2, got 5)
- HTTPRouteInvalidCrossNamespaceParentRef (expected 0, got >0)
- HTTPRouteInvalidParentRefSectionNameNotMatchingPort (expected 0, got >0)

## Root Cause

The `AttachedRouteTracker` counts **all** routes that reference a Gateway listener via `parentRef`, regardless of whether the route is actually **Accepted** by that listener. The Gateway API spec requires that `attachedRoutes` only count routes that are admitted/accepted.

`update_attached_route_tracker()` in `handlers/mod.rs` (line 88) registers every parentRef from every route into the tracker unconditionally. It does not check whether the route was accepted by the listener (namespace restrictions, sectionName/port mismatch, etc.). The Gateway handler's `update_status()` then reads `count_for_listener()` which returns the inflated count.

Note: The "expected 1, got 0" timing race in subtest 1 of GatewayWithAttachedRoutes is self-healing via requeue — the log shows it passes after retry. The **only real problem is over-counting**.

## Code Locations
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/attached_route_tracker.rs` — global tracker, stores/counts route attachments, has no concept of "accepted"
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs:88-117` — `update_attached_route_tracker()` registers ALL parentRefs unconditionally
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs:61-80` — `requeue_parent_gateways()` triggers Gateway re-status on route change
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs:236-244` — `on_change` calls tracker update then requeue
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs:375-386,498-506` — `update_status` reads `count_for_listener()` and writes to `ls.attached_routes`

## Proposed Fix

Modify `update_attached_route_tracker()` (or move the call) so it only registers parentRefs for which the route is **accepted** by the listener. Acceptance criteria:

- **Namespace allowed?** Check `listener.allowedRoutes.namespaces` (Same/All/Selector).
- **Kind allowed?** Check `listener.allowedRoutes.kinds`.
- **SectionName matches?** If specified, must match a real listener name.
- **Port matches?** If specified, must match the listener's port.

The cleanest approach: after `update_status` computes per-parentRef acceptance conditions, filter the attachments to only include accepted parentRefs, then call `update_route()` on the tracker.

Alternatively, add an `accepted: bool` flag to `Attachment` and only count accepted ones in `count_for_listener()`.

## Risk Assessment

Low-Medium. The tracker is a well-isolated module. The main risk is getting the acceptance logic right (it must mirror Gateway API spec semantics exactly for namespace/kind/sectionName/port filtering).

**Complexity note:** The route handler currently doesn't have access to the Gateway object when calling the tracker. Two approaches:
1. Move tracker update into `update_status` where acceptance is already computed (cleanest but requires refactoring the call site).
2. Pass acceptance results from `update_status` back to `on_change` to filter the tracker (more invasive).

## Benefit

Fixes 4 conformance tests:
- GatewayWithAttachedRoutes (subtest 2 of 3)
- HTTPRouteHostnameIntersection (AttachedRoutes subtest)
- HTTPRouteInvalidCrossNamespaceParentRef (0 Routes subtest)
- HTTPRouteInvalidParentRefSectionNameNotMatchingPort (0 Routes subtest)

## Verification
```bash
./scripts/orbstack-dev.sh rebuild c
./scripts/orbstack-dev.sh test --run-test GatewayWithAttachedRoutes
./scripts/orbstack-dev.sh test --run-test HTTPRouteHostnameIntersection
./scripts/orbstack-dev.sh test --run-test HTTPRouteInvalidCrossNamespaceParentRef
./scripts/orbstack-dev.sh test --run-test HTTPRouteInvalidParentRefSectionNameNotMatchingPort
```
