# AttachedRoutes Count Failures — Root Cause Analysis

## Root Cause

The `AttachedRouteTracker` counts **all** routes that reference a Gateway listener via `parentRef`, regardless of whether the route is actually **Accepted** by that listener. The Gateway API spec requires that `attachedRoutes` only count routes that are admitted/accepted by the listener.

Two distinct sub-problems:

1. **Over-counting (not-accepted routes included):** `update_attached_route_tracker()` in `handlers/mod.rs` (line 88) registers every parentRef from every route into the tracker unconditionally. It does not check whether the route was accepted by the listener (e.g., namespace restrictions, sectionName/port mismatch). The Gateway handler's `update_status()` then reads `count_for_listener()` which returns the inflated count.

2. **Timing / first-event miss (subtest "one attached route" gets 0):** The first subtest of `GatewayWithAttachedRoutes` expects 1 but gets 0. This suggests a race: the Gateway and Route are created near-simultaneously; the Gateway status is computed before the route's `on_change` has run and populated the tracker. The requeue mechanism (`requeue_parent_gateways`) fires from the route handler, but if the Gateway handler already ran and persisted status with count=0, the requeue may arrive too late or not trigger a re-persist (no-diff optimization).

## Affected Tests (4 tests, 6 subtests)

| Test | Subtest | Expected | Got | Sub-problem |
|------|---------|----------|-----|-------------|
| GatewayWithAttachedRoutes | Gateway listener should have one valid http routes attached | 1 | 0 | Timing race |
| GatewayWithAttachedRoutes | Gateway listener should have two valid http routes attached | 2 | 3 | Over-counting (includes `http-route-not-accepted`) |
| GatewayWithAttachedRoutes | AttachedRoutes set even when Gateway has unresolved refs | 1 | 1 | PASS |
| HTTPRouteHostnameIntersection | HTTPRoutes have to be counted in AttachedRoutes only if they are Accepted | 2 | 5 | Over-counting (includes non-intersecting routes) |
| HTTPRouteInvalidCrossNamespaceParentRef | Gateway should have 0 Routes attached | 0 | >0 | Over-counting (cross-ns route not allowed but still counted) |
| HTTPRouteInvalidParentRefSectionNameNotMatchingPort | Gateway should have 0 Routes attached | 0 | >0 | Over-counting (sectionName+port mismatch but still counted) |

## Code Locations

| File | Line(s) | Role |
|------|---------|------|
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/attached_route_tracker.rs` | all | Global tracker — stores/counts route attachments, has no concept of "accepted" |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs` | 88-117 | `update_attached_route_tracker()` — registers ALL parentRefs unconditionally |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs` | 61-80 | `requeue_parent_gateways()` — triggers Gateway re-status on route change |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs` | 236-244 | `on_change` — calls tracker update then requeue |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs` | 375-386, 498-506 | `update_status` — reads `count_for_listener()` and writes to `ls.attached_routes` |

## Proposed Fix

### Fix 1: Only track accepted routes (solves over-counting)

Modify `update_attached_route_tracker()` (or move the call) so it only registers parentRefs for which the route is **accepted** by the listener. This requires the route handler to evaluate acceptance criteria before updating the tracker:

- **Namespace allowed?** Check `listener.allowedRoutes.namespaces` (Same/All/Selector).
- **Kind allowed?** Check `listener.allowedRoutes.kinds`.
- **SectionName matches?** If specified, must match a real listener name.
- **Port matches?** If specified, must match the listener's port.

The simplest approach: after `update_status` computes per-parentRef acceptance conditions, filter the attachments set to only include accepted parentRefs, then call `update_route()` on the tracker.

Alternatively, add an `accepted: bool` flag to `Attachment` and only count accepted ones in `count_for_listener()`.

### Fix 2: Ensure requeue reliably updates count (solves timing race)

The "expected 1, got 0" case means the Gateway status was persisted before the route tracker was populated. Options:

- **Option A:** In the Gateway handler's `update_status`, if `attachedRoutes == 0` but there are known routes in the `gateway_route_index`, re-read the tracker (may still race).
- **Option B:** Ensure route `on_change` always requeues the parent Gateway, and Gateway handler always re-persists status on requeue even if conditions haven't changed. This may require removing a "no-diff skip" optimization in status persistence.
- **Option C (recommended):** The existing requeue mechanism should work — verify that the Gateway handler actually re-runs `update_status` and re-persists on requeue. The log shows the first check gets 0 but then 100ms later conditions match, suggesting the requeue did fire and the count updated — but the *first* status write had count=0. The conformance test eventually sees the correct count for subtest 1 (it passes after retry). Re-check: the log shows subtest 1 actually **passes** after the retry ("Gateway status listeners matched expectations"). The real failure is only subtest 2 (over-counting).

**Re-reading the log more carefully:** Subtest 1 shows "Expected AttachedRoutes to be 1, got 0" on first poll, then "Conditions matched expectations" + "Gateway status listeners matched expectations" — this means it **passed** after retry. Subtest 3 also passes. Only subtest 2 fails (expected 2, got 3 persistently for 10 seconds).

So the timing race is self-healing via requeue. The **only real problem is over-counting**.

## Risk / Complexity

- **Risk: Low-Medium.** The tracker is a well-isolated module. The main risk is getting the acceptance logic right (it must mirror Gateway API spec semantics exactly for namespace/kind/sectionName/port filtering).
- **Complexity: Medium.** The route handler currently doesn't have access to the Gateway object when calling the tracker. Two approaches:
  1. Move tracker update into `update_status` where acceptance is already computed (cleanest but requires refactoring the call site).
  2. Pass acceptance results from `update_status` back to `on_change` to filter the tracker (more invasive).

## Benefit

Fixing the over-counting issue directly resolves **4 conformance tests** (3 currently failing + GatewayWithAttachedRoutes subtest 2):

- GatewayWithAttachedRoutes (subtest 2 of 3)
- HTTPRouteHostnameIntersection (AttachedRoutes subtest)
- HTTPRouteInvalidCrossNamespaceParentRef (0 Routes subtest)
- HTTPRouteInvalidParentRefSectionNameNotMatchingPort (0 Routes subtest)

## Recommendation

**Priority: P1.** This is a targeted fix to the `update_attached_route_tracker` logic. The tracker should only count routes that are accepted by their target listener. The cleanest approach is to move the tracker update call into (or after) `update_status` on the route handler, where parentRef acceptance conditions have already been evaluated, and only register parentRefs with `Accepted: True`. The requeue mechanism already ensures the Gateway re-computes its status when route acceptance changes.
