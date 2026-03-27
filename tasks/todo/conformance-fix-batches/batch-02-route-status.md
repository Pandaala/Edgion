# Batch 2: Route Status Persistence

## Status: [ ] Not Started

## Tests to Fix
- HTTPRouteInvalidBackendRefUnknownKind (currently: FAIL individually)
- HTTPRouteInvalidNonExistentBackendRef (currently: FAIL individually)
- HTTPRouteInvalidCrossNamespaceBackendRef (currently: FAIL individually)
- HTTPRouteInvalidReferenceGrant (currently: FAIL individually)
- HTTPRoutePartiallyInvalidViaInvalidReferenceGrant (currently: FAIL individually)
- HTTPRouteReferenceGrant (currently: FAIL individually)

## Problem

All 6 tests fail because the controller correctly computes `ResolvedRefs` and `PartiallyInvalid` conditions internally, but the status is **not persisted to Kubernetes quickly enough** for the conformance test's 10-second polling window. Three possible causes:

1. **Race condition:** HTTPRoute is processed before referenced Service/ReferenceGrant arrives. Initial status write has stale conditions, and requeue may not trigger `update_status()`.
2. **Status write suppression:** `persist_k8s_status` in `resource_controller.rs` may skip writes when only status (not spec) changed, or may debounce excessively.
3. **Requeue skips re-evaluation:** When a Service appears after HTTPRoute processing, requeue detects no spec change and skips `update_status()`, leaving `ResolvedRefs` stale.

## Fix Steps

### Step 1: Trace the status persistence path

**File:** `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs`

1. Find `persist_k8s_status` and check if it has a "no-diff" optimization that compares old vs new status and skips the write. If so, ensure condition changes (not just spec changes) trigger a write.
2. Check if status writes are debounced with a timer > 2-3 seconds. If so, reduce or remove the debounce for status subresource patches.
3. Verify that after every `update_status()` call, `persist_k8s_status` is called unconditionally.

### Step 2: Ensure requeue triggers full re-evaluation

**Files:**
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs` lines 236-244 (`on_change`)
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant.rs` lines 87-101

When a Service or ReferenceGrant arrives and triggers an HTTPRoute requeue:
1. The requeued HTTPRoute must go through `parse()` (to re-evaluate `ref_denied`) AND `update_status()` (to recompute conditions).
2. If the requeue path has a "no spec change, skip processing" shortcut, add an exception for status-affecting dependencies (Service existence, ReferenceGrant).
3. Consider adding a `force_status_update` flag to requeue events triggered by dependency changes.

### Step 3: Verify condition-setting logic

**Files:**
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs` lines 99-134 -- `validate_backend_refs()` and `collect_ref_denied_errors()`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_utils.rs` lines 247-260 -- `resolved_refs_condition()`

Confirm the condition logic produces correct `ResolvedRefs=False` reasons:
- `BackendNotFound` for unknown kind / non-existent backend
- `RefNotPermitted` for cross-namespace without ReferenceGrant

These are believed to be correct already; verify by adding debug logging.

## Build & Deploy
```bash
# Controller only
./scripts/orbstack-dev.sh rebuild c
```

## Verify
```bash
# Run individual test(s)
./scripts/orbstack-dev.sh test --run-test HTTPRouteInvalidBackendRefUnknownKind
./scripts/orbstack-dev.sh test --run-test HTTPRouteInvalidNonExistentBackendRef
./scripts/orbstack-dev.sh test --run-test HTTPRouteInvalidCrossNamespaceBackendRef
./scripts/orbstack-dev.sh test --run-test HTTPRouteInvalidReferenceGrant
./scripts/orbstack-dev.sh test --run-test HTTPRoutePartiallyInvalidViaInvalidReferenceGrant
./scripts/orbstack-dev.sh test --run-test HTTPRouteReferenceGrant

# After all tests in this batch pass individually, run core batch:
./scripts/orbstack-dev.sh test --core
```

## Result
- [ ] Individual tests pass
- [ ] Core batch score improved
- Previous score: __/60
- New score: __/60
