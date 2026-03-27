# Batch 6: HTTPRouteHostnameIntersection attachedRoutes

## Status: [ ] Not Started

## Tests to Fix
- HTTPRouteHostnameIntersection (currently: FAIL individually -- attachedRoutes subtest: expected 2, got 5)

## Problem

The `AttachedRouteTracker` counts **all** routes that reference a Gateway listener via `parentRef`, regardless of whether the route is actually **Accepted** by that listener. The Gateway API spec requires that `attachedRoutes` only count admitted/accepted routes.

`update_attached_route_tracker()` in `handlers/mod.rs` (line 88) registers every parentRef from every route unconditionally without checking acceptance criteria (namespace restrictions, sectionName/port mismatch, kind filtering).

Note: This is the same root cause as the AttachedRoutes fix in `conformance-fixes/03-attached-routes-filtering.md`. The other tests affected by this bug (GatewayWithAttachedRoutes, HTTPRouteInvalidCrossNamespaceParentRef, HTTPRouteInvalidParentRefSectionNameNotMatchingPort) may already be passing or may be fixed by batch 2's route status work. This batch focuses on the HTTPRouteHostnameIntersection test specifically.

## Fix Steps

### Step 1: Filter attachedRoutes to only count accepted routes

**File:** `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs` lines 88-117

Modify `update_attached_route_tracker()` to only register parentRefs where the route is accepted. Acceptance requires:
- **Namespace allowed:** `listener.allowedRoutes.namespaces` permits the route's namespace (Same/All/Selector)
- **Kind allowed:** `listener.allowedRoutes.kinds` includes the route's kind
- **SectionName matches:** If specified in parentRef, must match a real listener name
- **Port matches:** If specified in parentRef, must match the listener's port

### Step 2: Choose implementation approach

**Option A (cleanest):** Move the tracker update into `update_status()` in the Gateway handler (`handlers/gateway.rs` lines 375-386) where acceptance is already computed. After computing per-parentRef acceptance, call `tracker.update_route()` with only accepted refs.

**Option B:** Pass acceptance results from `update_status` back to the route's `on_change` to filter before calling the tracker.

**Option A is recommended** -- the Gateway handler already has access to listener specs, namespace policies, and computed acceptance conditions.

### Step 3: Update `count_for_listener()` or tracker data model

**File:** `src/core/controller/conf_mgr/sync_runtime/resource_processor/attached_route_tracker.rs`

If using Option A, the tracker itself may not need changes -- just ensure it receives only accepted attachments. If using a flag-based approach, add an `accepted: bool` field to `Attachment` and filter in `count_for_listener()`.

## Build & Deploy
```bash
# Controller only
./scripts/orbstack-dev.sh rebuild c
```

## Verify
```bash
# Run individual test(s)
./scripts/orbstack-dev.sh test --run-test HTTPRouteHostnameIntersection
./scripts/orbstack-dev.sh test --run-test GatewayWithAttachedRoutes
./scripts/orbstack-dev.sh test --run-test HTTPRouteInvalidCrossNamespaceParentRef
./scripts/orbstack-dev.sh test --run-test HTTPRouteInvalidParentRefSectionNameNotMatchingPort

# After all tests in this batch pass individually, run core batch:
./scripts/orbstack-dev.sh test --core
```

## Result
- [ ] Individual tests pass
- [ ] Core batch score improved
- Previous score: __/60
- New score: __/60
