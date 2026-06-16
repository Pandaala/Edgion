---
name: status-management
description: update_status implementation rules and pitfalls — status write flow, field rules, and testing points for CRD status writeback.
---

# Status Management — update_status Implementation Rules

> For the location of status fields per resource, see [05-resources/SKILL.md](../05-resources/SKILL.md); this document covers only the status write flow, field rules, and testing points.
>
> The correct way to implement ProcessorHandler::update_status(), known pitfalls, and a review checklist.
> This document targets developers who need to implement or modify CRD status.

## 1. Status Position in the Processing Pipeline

```
process_resource(obj):
├── 4. validate()           → Vec<String>          ─┐
├── 5. preparse()           → Vec<String>           ├─ merged → all_errors
├── 6. parse()              → ProcessResult<K>     ─┘
├── 7. Extract old status   (for change detection)
├── 8. update_status(&mut obj, ctx, &all_errors) ◀── THIS IS THE FOCUS
├── 9. Extract new status   (for change detection)
├── 10. status_has_changed(old, new) → bool
└── 11. persist_k8s_status() / write_status_value() if changed
```

Key: `all_errors = validate() errors ∪ preparse() errors`, passed as `validation_errors`.

## 2. The Golden Rule

**`validation_errors` MUST influence the status.**

The processor merges errors from `validate()` and `preparse()` into a single
`all_errors` vec and passes it to `update_status()` as `validation_errors`.
If a handler ignores this parameter (uses `_validation_errors`), errors from
the validation phase will be logged but **never visible in the CRD status**,
making the resource appear healthy to users.

## 3. Conditions We Set

The Controller only sets conditions it can **accurately determine** from the
control plane:

| Condition | Meaning |
|-----------|---------|
| **Accepted** | Resource is syntactically/semantically valid and accepted by the Controller |
| **ResolvedRefs** | All references to other objects (Secrets, Services, etc.) are resolved |
| **PartiallyInvalid** | Some rules in the route have invalid backend refs but other rules are valid (HTTPRoute/GRPCRoute only) |

### Conditions We Do NOT Set: Programmed and Ready

**Decision: not implemented.**

`Programmed` and `Ready` require **data-plane feedback** to be accurate.
Our Controller-Gateway architecture uses async gRPC config sync where
Gateway actively pulls config via Watch/List. The Controller cannot confirm
whether the Gateway has loaded any specific configuration. Setting these
from the control plane would be misleading.

A Controller → Gateway HTTP Pull reverse-query scheme was once evaluated (with design completed down to a file-level implementation plan), but it would have introduced an entirely new communication path, proto changes, delay queues, and so on. The complexity was too high, so it was decided not to implement.

- The design document for that scheme has been deleted; no reference file is retained.
- Decision-of-record for not implementing: [skills/08-kubernetes/01-gateway-api/01-not-implemented.md](../../../skills/08-kubernetes/01-gateway-api/01-not-implemented.md)

## 4. Resource Categories and Status Patterns

### 4.1 Simple resources (flat status with conditions)

**Pattern:** GatewayClass, EdgionGatewayConfig, LinkSys, EdgionPlugins, EdgionStreamPlugins, BackendTLSPolicy, PluginMetaData

**Implementation split of Condition-type choice** — both styles coexist in this repo:

| Style | Condition library used | Helpers used | Resources |
|-------|-----------------------|--------------|-----------|
| Internal `types::resources::common::Condition` | `status_utils::{accepted_condition, condition_false, update_condition}` | GatewayClass, EdgionGatewayConfig, LinkSys, BackendTLSPolicy |
| `k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition` | Per-handler local `k8s_condition_true/false` + `update_k8s_condition` (with some duplication) | EdgionPlugins, EdgionStreamPlugins, EdgionAcme, PluginMetaData |

New resources should prefer `k8s_openapi::Condition` (aligned with the kube-rs ecosystem). The two styles of `update_condition`/`update_k8s_condition` behave equivalently: they refresh `last_transition_time` only when the status actually changes.

**GatewayClass special:** GatewayClassStatus also has `supported_features: Option<Vec<String>>`,
populated in `update_status()` with the full list of supported Gateway API features (matching
the conformance test declaration in `edgion-tests/conformance/conformance_test.go`).

```rust
fn update_status(&self, obj: &mut T, _ctx: &HandlerContext, validation_errors: &[String]) {
    let status = obj.status.get_or_insert_with(Default::default);

    if validation_errors.is_empty() {
        update_condition(&mut status.conditions, accepted_condition(generation));
    } else {
        update_condition(&mut status.conditions, condition_false(
            ACCEPTED, "Invalid", validation_errors.join("; "), generation,
        ));
    }

    // ResolvedRefs condition: driven by reference resolution checks
    // (handler-specific: check secrets, backends, etc.)
}
```

### 4.2 Per-parent resources (per-parent status)

**Pattern:** HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, EdgionTls

These resources have `parent_refs` and must set status per-parent:

```rust
fn update_status(&self, resource: &mut T, _ctx: &HandlerContext, validation_errors: &[String]) {
    let validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);

    // 1. Compute resolved_refs_errors (handler-specific)
    let resolved_refs_errors = ...;

    // 2. Per-rule validation (HTTPRoute/GRPCRoute only)
    let invalid_indices = find_invalid_rule_indices(route, route_ns);
    route.spec.invalid_rule_indices = if invalid_indices.is_empty() { None } else { Some(invalid_indices) };

    // 3. Per-parent loop
    if let Some(parent_refs) = &resource.spec.parent_refs {
        for parent_ref in parent_refs {
            let mut accepted_errors = validate_parent_ref_accepted(resource_ns, parent_ref, ...);
            accepted_errors.extend(validation_accepted_errors.clone());

            // Set Accepted and ResolvedRefs conditions
            set_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_refs_errors, gen);

            // Set PartiallyInvalid if some (but not all) rules are invalid
            if !invalid_indices.is_empty() && valid_rules > 0 {
                update_condition(conditions, condition_true(PARTIALLY_INVALID, RULE_INVALID, ...));
            }
        }
        retain_current_parent_statuses(&mut status.parents, parent_refs);
    } else {
        // ★ CRITICAL: clear stale parents when parent_refs removed
        status.parents.clear();
    }
}
```

#### PartiallyInvalid and invalid_rule_indices

For HTTPRoute and GRPCRoute, per-rule backend ref validation determines which rules
are invalid (all backend refs missing/denied). The result is stored in
`spec.invalid_rule_indices: Option<Vec<usize>>` and synced to the Gateway via gRPC.

**Controller side:** `find_invalid_rule_indices()` checks each rule — a rule is invalid
if it has backend refs and ALL of them are unreachable (wrong kind or service not found).
`ref_denied` backends are NOT treated as invalid — they stay in the route table so
Gateway can match and return 500. Three outcomes:

| Scenario | Accepted | PartiallyInvalid | invalid_rule_indices |
|----------|----------|------------------|---------------------|
| All rules valid | True | (not set) | None |
| Some rules invalid | True | True | Some([indices]) |
| All rules invalid | False (Invalid) | (not set) | Some([all indices]) |

**Gateway side:** During route compilation (`conf_handler_impl.rs`), the rule iteration
loop skips any rule whose index appears in `invalid_rule_indices`. The Gateway does NOT
need to know why the rule is invalid — it simply trusts the Controller's decision.

**Important:** Backend ref existence checks (service not found, wrong kind) must NOT be
in `validate()`. They belong in `update_status()` only, via `resolved_refs_errors` and
`find_invalid_rule_indices()`. Putting them in `validate()` would cause the entire route
to be Accepted: False even when only some rules have bad backends.

### 4.3 Gateway (hybrid: gateway-level + per-listener)

Gateway has both gateway-level conditions (Accepted, ListenersNotValid) and
per-listener conditions (Accepted, ResolvedRefs, Conflicted). All conditions
respect `validation_errors`.

### 4.4 EdgionAcme (hybrid: Accepted condition + lifecycle fields)

EdgionAcme has both a Gateway API-style `Accepted` condition and ACME-specific lifecycle fields (`phase` / `certificate_serial` / `last_renewal_time` / etc.):

- **Handler** (`EdgionAcmeHandler::update_status`) writes only `status.conditions[Accepted]`, reflecting the spec validation result
- **Background AcmeService** independently updates `phase` and the timestamp fields via Server-Side Apply (field manager `edgion-acme-controller`). The AcmeService SSA patch uses `skip_serializing_if = "Vec::is_empty"` to skip `conditions`, so it does not clear the condition written by the handler
- `last_failure_reason` is retained for legacy tooling

### 4.5 Resources without status

**Pattern:** Secret, Service, EndpointSlice, ReferenceGrant, ConfigMap

These use `DefaultHandler` or a handler with empty `update_status()`.

(EdgionData was previously in this list as `PluginMetaData`; it has been moved to 4.1 — Gateway API-style Accepted + ResolvedRefs; see the `update_status` section in [05-resources/15-edgion-data.md](../05-resources/15-edgion-data.md).)

### 4.6 Cross-resource reference resolution (new)

Several simple resources register outbound references during `parse` via indices such as `plugin_metadata_refs` / `SecretRefManager`, symmetrically clean them up during `on_delete` via `clear_refs_from`, and then look them up in the store during `update_status` to compute `ResolvedRefs`:

| Handler | Reference type | resolved via | Reason when unresolved |
|---------|---------------|--------------|-------------------------|
| EdgionPlugins | Secret | `SecretRefManager::get_dependencies(ep_key)` | `BackendNotFound` (`ResolvedRefsError::SecretNotFound`) |
| EdgionStreamPlugins | EdgionData (GlobalConnIpRestrictionOverride) | iterate GCIR entries in spec → `plugin_metadata_store.get(ns, name)` with matching type | `InvalidPluginMetaDataRef` |
| EdgionPlugins | EdgionData (RegionRouteOverride) | `plugin_metadata_store.get` via `overrideRef` with type guard | `InvalidPluginMetaDataRef` |
| EdgionTls | Secret | `secret_store.get` | `BackendNotFound` (`ResolvedRefsError::SecretNotFound`) |

When the referenced object changes, a reverse requeue is performed via `plugin_metadata_refs` / `SecretRefManager`, allowing the consumer's ResolvedRefs to self-heal. The reverse requeue encodes the kind in the `ep_key` format `"<Kind>:<ns>/<name>"`; `requeue_pm_referrers` uses `split_once(':')` to dispatch to the corresponding processor. (The old PM→PM cross-ref chain — ServiceRegionRoute → ClusterRegionRoute — was removed in the EdgionData refactor; `overrideRef` is now a direct EdgionPlugins → EdgionData reference.)

## 5. The set_parent_conditions_full Contract

```rust
pub fn set_parent_conditions_full(
    conditions: &mut Vec<Condition>,
    accepted_errors: &[AcceptedError],          // per-parent: controls Accepted
    resolved_refs_errors: &[ResolvedRefsError],  // resource-level: controls ResolvedRefs
    observed_generation: Option<i64>,
)
```

- If `accepted_errors` is empty → Accepted: True
- If `accepted_errors` is non-empty → Accepted: False (uses first error's reason)
- `resolved_refs_errors` → ResolvedRefs: True/False (independent of Accepted)

Both conditions are always explicitly set, preventing stale values.

To surface `validation_errors`, the handler must convert them to
`AcceptedError` via `AcceptedError::from_validation_errors()` before calling.

## 6. Naming Conventions

The status utility functions use **resource-neutral naming**:

| Function | Purpose |
|----------|---------|
| `accepted_condition()` | Message: "Resource accepted" |
| `set_parent_conditions()` | Wrapper with empty accepted_errors |
| `set_parent_conditions_full()` | Full condition setter |
| `AcceptedError::NotAllowedByListeners` | Field: `resource_ns` (not `route_ns`) |

EdgionTls handler uses `resource_ns` for namespace variable (not `route_ns`).

The `RouteParentStatus` struct from `http_route.rs` is reused by EdgionTls
because the per-parent status pattern is identical to Gateway API routes.
This is a type-level reuse; the struct name does not appear in K8s status YAML.

## 7. Status Persistence

### K8s mode
- `persist_k8s_status()` → `api.patch_status(name, &params, &Patch::Merge(...))`
- Uses `DynamicObject` + `ApiResource` to avoid generic constraints
- Guarded by leader election: `status_changed && can_write_status`

### FileSystem mode
- `status_handler.write_status_value(kind, &key, &status_value)` → `.status` YAML file
- Guarded by: `status_changed` only (no leader check)

### Change detection — Semantic Status Diff

**Every status write is a K8s API call.** Minimizing unnecessary writes is
critical for scalability.

Status is serialized to JSON before and after `update_status()`. The comparison
uses **semantic diffing** (`status_diff::status_semantically_changed`) rather
than raw string comparison. This ignores non-semantic fields like
`lastTransitionTime` so that re-processing a resource (e.g., after Controller
restart or cross-resource requeue) does NOT trigger a write when the logical
state is unchanged.

#### How it works

`status_has_changed(kind, old, new)` in `processor.rs` delegates to
`status_diff::status_semantically_changed(kind, old_json, new_json)`, which:

1. Deserializes both JSON strings into `serde_json::Value`
2. Routes to a kind-specific comparator based on the resource `kind` string
3. Compares only **semantic fields** of conditions:
   - `type`, `status`, `reason`, `message`, `observedGeneration` — compared
   - `lastTransitionTime` — **ignored**

#### Kind-specific comparators

| Category | Kinds | Comparator | What it compares |
|----------|-------|------------|------------------|
| **Condition-only** | GatewayClass, EdgionGatewayConfig, LinkSys, EdgionPlugins, EdgionStreamPlugins, PluginMetaData | `conditions_only_status_equal` | `status.conditions[]` |
| **Per-parent** | HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, EdgionTls | `parents_status_equal` | `status.parents[]` matched by parentRef identity |
| **Per-ancestor** | BackendTLSPolicy | `ancestors_status_equal` | `status.ancestors[]` matched by ancestorRef identity |
| **Hybrid** | Gateway | `gateway_status_equal` | `addresses` + gateway-level `conditions` + `listeners[]` (matched by name) |
| **Hybrid (lifecycle + conditions)** | EdgionAcme | Direct `Value` equality | All fields compared verbatim — conditions and lifecycle fields together |
| **Unknown** | Future resources | `conditions_only_status_equal` | Safe fallback |

#### Key scenarios this optimizes

1. **Controller restart**: All resources re-processed with fresh timestamps →
   semantic diff detects "no change" → zero K8s API writes.
2. **Cross-resource requeue**: Gateway requeued due to Secret arrival but
   nothing actually changed → no write.
3. **Periodic re-validation**: Resources re-evaluated but state is stable →
   no write.

#### Adding a new resource kind

When adding a new resource with status, add its `kind` string to the
appropriate match arm in `status_diff::status_semantically_changed()`. If the
status structure doesn't fit any existing pattern, create a new comparator
function.

#### Important edge cases

- `Empty → Present("{}")` is still treated as a change (triggers initial write)
- Invalid JSON on either side → always treated as changed (safe fallback)
- Condition arrays in different order → correctly handled (matched by `type`)
- Parent/listener arrays in different order → correctly handled (matched by identity)

## 8. Stale Status Cleanup

### Per-parent resources
Old parent statuses are cleaned up in two ways:

1. **Parent removed from spec**: `retain_current_parent_statuses()` removes
   parent statuses whose `parent_ref` no longer matches any ref in the spec.
2. **`parent_refs` removed entirely (set to None)**: The `else` branch on the
   `if let Some(parent_refs)` clears `status.parents` completely.

### On Controller restart
The Controller re-processes all resources from the K8s store. Each resource
carries its existing status; `update_status()` uses `update_condition()` to
replace conditions in-place (preserving `last_transition_time` when status
hasn't actually changed). Since `set_parent_conditions_full` always sets both
standard conditions, no stale condition values survive re-processing.

## 9. Review Checklist for update_status Implementations

- [ ] **Uses validation_errors**: parameter is NOT prefixed with `_`
- [ ] **Accepted condition reflects validation_errors**: non-empty → Accepted: False
- [ ] **ResolvedRefs reflects reference resolution**: missing Secret/Service → False
- [ ] **Per-parent status** (for per-parent resources): each parent_ref has its own status
- [ ] **retain_current_parent_statuses**: stale parent statuses are cleaned up
- [ ] **parent_refs None → clear parents**: `else` branch calls `status.parents.clear()`
- [ ] **Status struct serialization**: `skip_serializing_if` does not hide error information
- [ ] **No duplicate checks**: if validate() already checks something, update_status uses `validation_errors` instead of re-checking
- [ ] **No backend ref existence in validate()**: service-not-found and kind checks go in update_status only (via resolved_refs_errors + find_invalid_rule_indices), not in validate()
- [ ] **No "route" in non-route contexts**: use `resource_ns`, `set_parent_conditions_full`, etc.
- [ ] **No Programmed/Ready conditions**: permanently not implemented — requires data-plane feedback that is not available in current architecture
- [ ] **Status diff registration**: new resource kind added to `status_diff::status_semantically_changed()` match arms
- [ ] **Status diff tests**: new resource kind has test cases in `status_diff.rs` covering no-change and changed scenarios

## 10. Historical Fixes

### 2026-03-25: PartiallyInvalid condition and invalid_rule_indices
Added per-rule backend ref validation for HTTPRoute and GRPCRoute. When some rules
have all-invalid backend refs, the Controller sets `PartiallyInvalid: True` and stores
the invalid indices in `spec.invalid_rule_indices`. The Gateway skips these rules during
route compilation. Also added `supported_features` to GatewayClassStatus.

Key changes:
- `find_invalid_rule_indices()` in both http_route.rs and grpc_route.rs handlers
- `invalid_rule_indices: Option<Vec<usize>>` field in HTTPRouteSpec and GRPCRouteSpec
- Gateway skip logic in `conf_handler_impl.rs` for HTTP and gRPC route compilation
- Moved backend ref existence checks OUT of `validate()` into `update_status()` only
- `PARTIALLY_INVALID` condition type and `RULE_INVALID` reason in status_utils.rs

### 2026-03-18: Semantic status diff for minimized K8s API writes
Replaced raw JSON string comparison with semantic status diffing
(`status_diff.rs`). The new `status_semantically_changed()` function ignores
`lastTransitionTime` and uses kind-specific comparators to avoid false-positive
status writes. This eliminates unnecessary K8s API calls on Controller restart,
cross-resource requeue, and periodic re-validation.

Key changes:
- Added `status_diff.rs` module with layered comparison: condition-level →
  composite (parents/ancestors/listeners) → top-level kind dispatch.
- `status_has_changed()` in `processor.rs` now takes `kind` parameter and
  delegates to `status_diff::status_semantically_changed()`.
- 64 unit tests covering all resource categories and edge cases.

### 2026-03-17: validation_errors ignored
All per-parent handlers now correctly use `validation_errors` via
`AcceptedError::from_validation_errors()`. EdgionTls CRD schema corrected
(`status.parents` instead of `status.condition`).

### 2026-03-17: Naming cleanup
Removed "route" terminology from shared utilities: `set_route_parent_conditions_full`
→ `set_parent_conditions_full`, `"Route accepted"` → `"Resource accepted"`,
`route_ns` → `resource_ns` in EdgionTls handler.

### 2026-03-17: parent_refs removal cleanup
All per-parent handlers now clear `status.parents` when `parent_refs` is None,
preventing stale parent statuses after spec changes.

### 2026-03-17: Removed Programmed and Ready conditions
Removed `Programmed` and `Ready` conditions from all resource handlers.
These conditions require data-plane feedback to be accurate, but the
Controller-Gateway architecture uses async gRPC sync without a status-back
channel. Setting them from the control plane was misleading. Only `Accepted`
and `ResolvedRefs` are now set — conditions the Controller can accurately
determine.

## 11. Related

- [02-resource-controller.md](03-config-center/02-kubernetes/02-resource-controller.md) — Status persistence and leader guard
- [03-resource-system.md](../00-common/03-resource-system.md) — Resource system and preparse
- [00-guide.md](09-add-new-resource/00-guide.md) — Adding new resource types
- [11-conf-handler-guidelines.md](../02-gateway/11-conf-handler-guidelines.md) — ConfHandler (gateway side) guidelines
