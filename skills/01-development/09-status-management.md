# Status Management — update_status 实现规范

> ProcessorHandler::update_status() 的正确实现方式、已知陷阱和审查清单。
> 本文档面向需要实现或修改 CRD status 的开发者。

## 1. Status 在处理管线中的位置

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

### Conditions We Do NOT Set (Yet): Programmed and Ready

`Programmed` and `Ready` require **data-plane feedback** to be accurate.
Our Controller-Gateway architecture uses async gRPC config sync without a
status-back channel. The Controller cannot know whether the Gateway has
actually loaded the configuration or is serving traffic. Setting these from
the control plane would be misleading, so they are intentionally omitted.

A planned enhancement will introduce a **data-plane status query mechanism**
where the Controller queries the Gateway's Admin API for actual runtime state.
See [tasks/todo/data-plane-status-feedback.md](../../tasks/todo/data-plane-status-feedback.md)
for the full design. Key points:

- Controller → Gateway HTTP pull (not part of gRPC config sync)
- Delayed query (10s after resource change), max 2 retries
- Only the status query worker sets Programmed/Ready; handlers never set them directly
- Generation check prevents stale updates

## 4. Resource Categories and Status Patterns

### 4.1 Simple resources (flat status with conditions)

**Pattern:** GatewayClass, EdgionGatewayConfig, LinkSys, EdgionPlugins, EdgionStreamPlugins, BackendTLSPolicy

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

    // 2. Per-parent loop
    if let Some(parent_refs) = &resource.spec.parent_refs {
        for parent_ref in parent_refs {
            let mut accepted_errors = validate_parent_ref_accepted(resource_ns, parent_ref, ...);
            accepted_errors.extend(validation_accepted_errors.clone());

            // Set Accepted and ResolvedRefs conditions
            set_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_refs_errors, gen);
        }
        retain_current_parent_statuses(&mut status.parents, parent_refs);
    } else {
        // ★ CRITICAL: clear stale parents when parent_refs removed
        status.parents.clear();
    }
}
```

### 4.3 Gateway (hybrid: gateway-level + per-listener)

Gateway has both gateway-level conditions (Accepted, ListenersNotValid) and
per-listener conditions (Accepted, ResolvedRefs, Conflicted). All conditions
respect `validation_errors`.

### 4.4 EdgionAcme (lifecycle status)

Uses `phase`, `last_failure_reason`, etc. No standard conditions.

### 4.5 Resources without status

**Pattern:** Secret, Service, EndpointSlice, Endpoint, ReferenceGrant, PluginMetaData, ConfigMap

These use `DefaultHandler` or a handler with empty `update_status()`.

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

### Change detection
Status is serialized to JSON before and after `update_status()` and compared
as strings. Only actual changes trigger persistence.

Important: if `status` serializes to `{}` (e.g., `EdgionTlsStatus { parents: vec![] }`
with `skip_serializing_if = "Vec::is_empty"`), the `status` field in JSON becomes
`"status":{}` which is `StatusExtractResult::Present("{}")`. If the K8s object
originally had `"status": null` or no status field, this IS a change and will
trigger a write — but the written status is effectively empty, providing no
useful information to users.

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
- [ ] **No "route" in non-route contexts**: use `resource_ns`, `set_parent_conditions_full`, etc.
- [ ] **No Programmed/Ready conditions**: these require data-plane feedback and must not be set from the control plane

## 10. Historical Fixes

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

- [02-resource-controller.md](../00-architecture/01-config-center/02-kubernetes/02-resource-controller.md) — Status persistence and leader guard
- [08-resource-system.md](../00-architecture/08-resource-system.md) — Resource system and preparse
- [00-add-new-resource.md](00-add-new-resource.md) — Adding new resource types
- [08-conf-handler-guidelines.md](08-conf-handler-guidelines.md) — ConfHandler (gateway side) guidelines
