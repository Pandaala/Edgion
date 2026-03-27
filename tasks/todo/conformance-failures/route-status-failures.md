# Route Status Failures — 6 HTTPRoute Backend Ref / ReferenceGrant Tests

## Summary

All 6 tests fail because the controller correctly computes `ResolvedRefs` and `PartiallyInvalid` conditions internally, but the status is **not persisted to Kubernetes quickly enough** (or at all) for the conformance test's 10-second polling window. The root cause is a **status write timing / reliability issue**, not missing logic.

---

## Test-by-Test Analysis

### Group A: ResolvedRefs=False expected (4 tests)

| Test | What It Does | Expected Status |
|------|-------------|----------------|
| **HTTPRouteInvalidBackendRefUnknownKind** | backendRef with `kind: SomeNonexistentKind` | `ResolvedRefs=False`, reason=`InvalidKind` |
| **HTTPRouteInvalidNonExistentBackendRef** | backendRef pointing to a Service that does not exist | `ResolvedRefs=False`, reason=`BackendNotFound` |
| **HTTPRouteInvalidCrossNamespaceBackendRef** | backendRef to a Service in another namespace, no ReferenceGrant | `ResolvedRefs=False`, reason=`RefNotPermitted` |
| **HTTPRouteInvalidReferenceGrant** | backendRef to another namespace, ReferenceGrant exists but does not match (wrong from/to) | `ResolvedRefs=False`, reason=`RefNotPermitted` |

**Controller logic (all present and correct):**

- `validate_backend_refs()` in `route_utils.rs:99-122` checks kind != "Service" -> `InvalidKind`, and service not found -> `BackendNotFound`.
- Cross-namespace ref denied refs are tracked via `ref_denied` field on `BackendRef` during `parse()` (http_route.rs:183-220), then collected by `collect_ref_denied_errors()` -> `RefNotPermitted`.
- `set_parent_conditions_full()` correctly maps these `ResolvedRefsError` variants to `ResolvedRefs=False` with the correct reason string.

### Group B: PartiallyInvalid expected (1 test)

| Test | What It Does | Expected Status |
|------|-------------|----------------|
| **HTTPRoutePartiallyInvalidViaInvalidReferenceGrant** | Two rules: one with valid backendRef, one with cross-ns backendRef denied by ReferenceGrant | `Accepted=True`, `ResolvedRefs=False` (reason=`RefNotPermitted`), `PartiallyInvalid=True` |

**Controller logic (present and correct):**

- `find_invalid_rule_indices()` in http_route.rs:402-464 identifies rules where ALL backend refs are unreachable (wrong kind, missing service, or ref denied).
- `update_status()` at lines 322-345 sets `PartiallyInvalid=True` when `invalid_indices` is non-empty and `valid_rules > 0`.

### Group C: ReferenceGrant allows cross-namespace ref (1 test)

| Test | What It Does | Expected Status |
|------|-------------|----------------|
| **HTTPRouteReferenceGrant** | backendRef to another namespace, valid ReferenceGrant exists | `ResolvedRefs=True`, traffic reaches the cross-namespace backend |

**Controller logic (present and correct):**

- `is_cross_ns_ref_allowed()` checks the ReferenceGrantStore for a matching grant.
- When allowed, `ref_denied` is cleared (http_route.rs:205-206), so no `RefNotPermitted` error is generated.
- This test may also fail on the data-plane side (traffic must actually reach the cross-namespace backend).

---

## Root Cause

The condition-setting logic is **fully implemented and correct**. The 10-second timeout failures point to one or more of these issues:

### 1. Status write timing — race between route processing and Service/ReferenceGrant arrival

When the conformance test creates an HTTPRoute simultaneously with (or before) the referenced Service or ReferenceGrant, the controller may process the HTTPRoute before the Service or ReferenceGrant is in the processor registry. This creates a **false positive** `BackendNotFound` or `RefNotPermitted` that may or may not be corrected by a requeue.

**Evidence:** The controller registers service backend refs for requeue (`register_service_backend_refs`), and ReferenceGrant changes dispatch revalidation events. However, the initial status write with a stale/incorrect condition may be what the test sees — or the requeue may not trigger a fresh `update_status()` call.

### 2. Status persistence latency or suppression

The controller uses `persist_k8s_status` (in `resource_controller.rs`) to write status back to the Kubernetes API. If status writes are batched, debounced, or skipped when the resource content hasn't changed (only status differs), the conditions may never reach the API server within 10 seconds.

### 3. Requeue not triggering status re-evaluation for "no-change" updates

When a Service appears after the HTTPRoute is already processed, the requeue may detect no spec change and skip `update_status()`. The `ResolvedRefs` condition would remain stale.

---

## Code Locations

| File | Lines | Purpose |
|------|-------|---------|
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs` | 263-352 | `update_status()` — sets Accepted, ResolvedRefs, PartiallyInvalid |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs` | 105-222 | `parse()` — marks `ref_denied` on cross-ns backend refs |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs` | 402-464 | `find_invalid_rule_indices()` — per-rule validation |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs` | 99-122 | `validate_backend_refs()` — kind + existence checks |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs` | 125-134 | `collect_ref_denied_errors()` — RefNotPermitted errors |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_utils.rs` | 247-260 | `resolved_refs_condition()` — builds the ResolvedRefs condition |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant.rs` | 87-101 | `on_change()` — dispatches revalidation event |
| `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs` | — | `persist_k8s_status` — writes status to K8s API |

---

## Proposed Fix Approach

### Step 1: Verify status persistence path (investigate first)

Check `resource_controller.rs` to confirm:
- Status is written after every `update_status()` call, not just on spec changes.
- The status subresource patch is not suppressed by a "no-diff" optimization that ignores condition changes.
- Status writes are not excessively debounced (> a few seconds).

### Step 2: Ensure requeue triggers full re-evaluation

When a Service or ReferenceGrant arrives/changes and triggers an HTTPRoute requeue:
- The requeued HTTPRoute must go through `parse()` (to re-evaluate `ref_denied`) AND `update_status()` (to recompute conditions).
- If the requeue path skips `update_status()` because the route spec hasn't changed, add a forced status re-evaluation.

### Step 3: Add integration tests

Add tests that:
1. Create an HTTPRoute with a non-existent backend, verify `ResolvedRefs=False`.
2. Create the Service, verify `ResolvedRefs` transitions to `True`.
3. Create a cross-namespace HTTPRoute without ReferenceGrant, verify `ResolvedRefs=False`.
4. Add a ReferenceGrant, verify `ResolvedRefs` transitions to `True`.

---

## Risk / Complexity

- **Risk: Low-Medium.** The condition logic is correct; the fix is in the status persistence/requeue plumbing.
- **Complexity: Medium.** Requires tracing the full lifecycle from requeue -> parse -> update_status -> persist to find where the chain breaks. The fix itself is likely small (ensure status writes on requeue, remove no-diff suppression for status).
- **Regression risk: Low.** These are status-only changes; no data-plane impact.

---

## Benefit

Fixes **6 core conformance tests** (of 10 remaining core failures):
- HTTPRouteInvalidBackendRefUnknownKind
- HTTPRouteInvalidCrossNamespaceBackendRef
- HTTPRouteInvalidNonExistentBackendRef
- HTTPRouteInvalidReferenceGrant
- HTTPRoutePartiallyInvalidViaInvalidReferenceGrant
- HTTPRouteReferenceGrant

---

## Recommendation

**Priority: High.** These 6 tests represent 60% of remaining core failures. The logic is already implemented — the gap is in the status persistence pipeline. Investigate `resource_controller.rs` and the requeue path first; the fix is likely a small change to ensure `update_status()` + `persist_k8s_status()` run on every requeue, not just on spec changes.
