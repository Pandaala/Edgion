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
├── Seed backend status, then capture old status before handler mutation
├── K8s shared resources: project only this deployment's observation into native status
├── validate()             → Vec<String>          ─┐
├── preparse()             → Vec<String>           ├─ merged → all_errors
├── parse()                → ProcessResult<K>     ─┘
├── update_status(&mut obj, ctx, &all_errors) ◀── THIS IS THE FOCUS
├── Separate local status from the ownership-preserving persistence payload
├── finalize_transport()
├── Extract new status; status_has_changed(old, new) → bool
├── on_change()
├── Publish processed object to ServerCache
├── after_publish()
└── Return result; outer worker conditionally persists status
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
| **PartiallyInvalid** | Part of a partially-resolvable collection is invalid while the rest stays usable. Not route-exclusive: route rules with invalid backend refs, inline CIDR / IpList entries, an EdgionBackend credential pool with unresolved credentials — see [§4.2](#partiallyinvalid-on-non-route-resources) |
| **Conflicted** | Listener or policy conflicts detected by the Controller |
| **Programmed** | Gateway/Gateway Listener processing state derived by the Gateway handler |

### Ready Is Not Set; Programmed Is Gateway-Specific

The Controller does not set `Ready`, because it has no per-resource data-plane
acknowledgement. It does set Gateway-level and listener-level `Programmed` in
`GatewayHandler`: listener `Programmed=True` requires Accepted, no conflict, and
resolved references; the gateway-level condition reflects controller-side
processing/config publication semantics. Do not generalize that condition to
other resources without a resource-specific contract.

A Controller → Gateway HTTP Pull reverse-query scheme was once evaluated (with design completed down to a file-level implementation plan), but it would have introduced an entirely new communication path, proto changes, delay queues, and so on. The complexity was too high, so it was decided not to implement.

- The design document for that scheme has been deleted; no reference file is retained.
- Decision-of-record for not implementing: [skills/08-kubernetes/01-gateway-api/01-not-implemented.md](../../../skills/08-kubernetes/01-gateway-api/01-not-implemented.md)

## 4. Resource Categories and Status Patterns

### 4.1 Simple resources (flat status with conditions)

**Pattern:** GatewayClass, EdgionGatewayConfig, LinkSys, EdgionPlugins, EdgionStreamPlugins, EdgionConfigData

This is the **local** condition shape. In Kubernetes, EdgionPlugins,
EdgionStreamPlugins, EdgionBackend and LinkSys persist complete observations in
`status.controllers[]`, keyed by `(controllerName, gatewayClassName)`. Each entry
contains `gatewayClassUid` and a nested native `status` payload. UID proves the
class incarnation, not a new list key. LinkSys lifecycle fields belong inside
that payload too; there is no aggregate top-level status that competing
deployments can overwrite. FileSystem/etcd and conf_sync retain native status.

The shared `ControllerStatus<T>` envelope, projection/merge in
`status_observation`, and `status_pipeline` adapters own this mechanism.
Handlers consume only projected native values. Missing ownership, invalid
observations or the 32-entry cap suppress persistence, not local processing.
An existing owner can update at the cap; a new owner cannot evict another.
Inactive observations remain until committed GatewayClass deletion or identity
replacement proves them obsolete. Absence requires a completed, unfiltered
GatewayClass LIST; filtered, unwatched and initializing views cannot prove it.

These Edgion status structs use
`edgion_resources::resources::common::Condition` and the shared
`status_utils::{condition_true, condition_false, update_condition}` helpers.
Choose a condition type from the resource API contract and reuse the matching
shared helpers; do not introduce handler-local duplicates based on the old split.

**Not this pattern:** BackendTLSPolicy and EdgionBackendTrafficPolicy report
GEP-2648 **per-ancestor** status (`status.ancestors[]`, one `PolicyAncestorStatus`
per ancestor Gateway) instead of a single flat `conditions[]` — see the
per-ancestor comparator row in [§7](#7-status-persistence) and
[20-edgion-backend-traffic-policy.md](../../02-features/03-resources/20-edgion-backend-traffic-policy.md#status-schema).

**GatewayClass special:** GatewayClassStatus also has `supported_features: Option<Vec<SupportedFeature>>`
(each wire entry is an object with a `name` field),
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

            // Reuse only the current controller's canonical parent entry.
            let parent = parent_status_for_update(
                &mut status.parents, parent_ref, resource_ns, &self.controller_name,
            );
            let conditions = &mut parent.conditions;
            set_parent_conditions_full(conditions, &accepted_errors, &resolved_refs_errors, gen);

            // Set PartiallyInvalid if some (but not all) rules are invalid
            if !invalid_indices.is_empty() && valid_rules > 0 {
                update_condition(conditions, condition_true(PARTIALLY_INVALID, RULE_INVALID, ...));
            }
        }
        retain_current_parent_statuses(&mut status.parents, parent_refs, resource_ns, &self.controller_name);
    } else {
        // Remove our stale entries, retaining other controllers' observed status.
        retain_current_parent_statuses(&mut status.parents, &[], resource_ns, &self.controller_name);
    }
}
```

Parent identity uses the shared `ParentReference::canonical_identity`: defaulted group/kind,
effective namespace, name, section and port. A status additionally belongs to `controllerName`.
The five Route handlers stamp `spec.resolvedStatusController` in `finalize_transport` from the
configured handler identity, after status derivation and before conf_sync publication. Gateway
route compilation and plugin publication consume that same-resource owner; EdgionTls keeps its
separate certificate/proof path. See the [status ownership guard](../../04-review/status/regression-guards/canonical-parent-status-ownership.md).

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

#### PartiallyInvalid on non-route resources

`PartiallyInvalid` is not route-exclusive: any resource whose spec holds a collection that
can be *partially* resolved may report it. `invalid_rule_indices` is the route-specific
carrier, not part of the condition's contract — a non-route resource sets the condition
without one, and reaches it through the shared
`status_utils::partially_invalid_condition(entry_message, generation)` helper
(`Some(msg)` → True/`RuleInvalid`, `None` → False/`AllRulesValid`).

One documented reason outside that helper: `HTTPRoute::update_status` builds its
`PartiallyInvalid` inline, and when an AI attachment/retry-cap violation already rejected
the whole route (`Accepted=False/UnsupportedValue`) it clears a stale True to
False/`UnsupportedValue` instead of False/`AllRulesValid` — the generic "all rules are
valid" wording would contradict the `Accepted=False` beside it. See
`skills/04-review/ai-routing/regression-guards/ai-rule-accepted-escalates-only-when-nothing-serves.md`.

Flat-status resources already using that helper: `EdgionPlugins` and `EdgionStreamPlugins`
(bad inline IpRestriction / GlobalConnectionIpRestriction profile CIDR entries) and
`EdgionConfigData` (bad IpList entries) — all three from `update_status()`, all with the
shared `RuleInvalid` reason.

**`EdgionBackend` differs on two axes** and is the reference case for both:

1. **Resource-specific reason.** It is the first emitter to use its own reason,
   `InvalidCredentials` (`status_utils.rs`), instead of the shared `RuleInvalid`.
2. **One status publication phase.** Credential resolution records a reconcile
   outcome in `parse()`; `update_status()` emits Accepted, ResolvedRefs and
   PartiallyInvalid together from that outcome and validation errors.

It sets `PartiallyInvalid=True` when `resolved_count > 0` and at least one provider
credential failed to resolve. The message is
`Omitted credentials (N): <alias>(<reason>), …`, where `<alias>` is the credential name
passed through `bounded_status_alias` — substituted with `credential[<index>]` when the name
is empty, over `AI_CREDENTIAL_ALIAS_MAX_BYTES`, or not restricted-charset, so a malformed
name cannot corrupt the status. That sanitizer is defense-in-depth for the lenient-admission
path: `validate()` already rejects such names, but it is advisory — `parse()` runs on a
resource with validation errors, so the substitution branch is reachable. Note it bounds each
*alias*, not the *number* of entries (one per failed credential). Otherwise it falls back to
`partially_invalid_condition(None, ..)` and the outcome is carried by `ResolvedRefs`:

| Scenario | ResolvedRefs | PartiallyInvalid |
|----------|--------------|------------------|
| All credentials resolved | True | False (`AllRulesValid`) |
| Some resolved, some failed | False | True (`InvalidCredentials`) |
| None resolved | False | False (`AllRulesValid`) |

New resources may follow this precedent, but they still draw from the fixed condition
vocabulary — a partially-resolvable collection is a reason to reuse `PartiallyInvalid`, not
to invent a new condition type.

### 4.3 Gateway (hybrid: gateway-level + per-listener)

Gateway has both gateway-level conditions (Accepted, ListenersNotValid) and
per-listener conditions (Accepted, ResolvedRefs, Conflicted, Programmed). The
Gateway also carries gateway-level Programmed. All conditions
respect `validation_errors`.

### 4.4 EdgionAcme (hybrid: Accepted condition + lifecycle fields)

EdgionAcme has both a Gateway API-style `Accepted` condition and ACME-specific lifecycle fields (`phase` / `certificate_serial` / `last_renewal_time` / etc.):

- **Handler parse** resolves the HTTP-01 attachment and account Secret and writes `ResolvedRefs`
- **Handler update_status** manages `Accepted`, reflecting the spec validation result
- **Handler after_publish** sends a notification only for a meaningful change from the previous processed cache (UID/generation or ResolvedRefs status/reason). It must not deduplicate against raw persisted conditions or notify before publishing recovered eligibility; see the [ACME notification guard](../../04-review/acme/regression-guards/resource-change-notification-gating.md).
- **Background AcmeService** independently updates `phase` and the timestamp fields via Server-Side Apply (field manager `edgion-acme-controller`). The AcmeService SSA patch uses `skip_serializing_if = "Vec::is_empty"` to skip `conditions`, so it does not clear the condition written by the handler
- `last_failure_reason` still exists for old tooling, but that is pre-release
  removal debt. New status consumers must use conditions; do not extend the
  text field's contract.

The generic handler persists the whole serialized status snapshot, including nonempty
observed lifecycle/challenge fields. Its raw-input resourceVersion precondition prevents
rollback of a newer service write. This is distinct from field ownership: the handler and
service still overlap on `last_failure_reason`. See [ACME writer boundaries](../05-resources/17-edgion-acme.md).

### 4.5 Resources without status

**Pattern:** Secret, Service, EndpointSlice, ReferenceGrant, ConfigMap

These use `DefaultHandler` or a handler with empty `update_status()`.

(EdgionConfigData was previously in this list as `PluginMetaData`; it has been moved to 4.1 — Gateway API-style Accepted + ResolvedRefs; see the `update_status` section in [05-resources/15-edgion-config-data.md](../05-resources/15-edgion-config-data.md).)

### 4.6 Cross-resource reference resolution (new)

Several simple resources register outbound references during `parse` via indices such as `edgion_config_data_refs` / `SecretRefManager`, symmetrically clean them up during `on_delete` via `clear_refs_from`, and then look them up in the store during `update_status` to compute `ResolvedRefs`:

| Handler | Reference type | resolved via | Reason when unresolved |
|---------|---------------|--------------|-------------------------|
| EdgionPlugins | Secret / ConfigMap credential material | `SecretRefManager::get_dependencies(ep_key)` with kind-preserving ConfigMap keys | `SecretNotFound` / `ConfigMapNotFound` |
| EdgionStreamPlugins | EdgionConfigData (Selector) | iterate GCIR entries' `activeProfileRef` → `edgion_config_data_store.get(ns, name)` with matching `Selector` type | `InvalidEdgionConfigDataRef` |
| EdgionPlugins | EdgionConfigData (RegionRouteOverride) | `edgion_config_data_store.get` via `overrideRef` with type guard | `InvalidEdgionConfigDataRef` |
| EdgionTls | Secret | `secret_store.get` | `SecretNotFound` |

When the referenced object changes, a reverse requeue is performed via `edgion_config_data_refs` / `SecretRefManager`, allowing the consumer's ResolvedRefs to self-heal. The reverse requeue encodes the kind in the `ep_key` format `"<Kind>:<ns>/<name>"`; `requeue_pm_referrers` uses `split_once(':')` to dispatch to the corresponding processor. (The old PM→PM cross-ref chain — ServiceRegionRoute → ClusterRegionRoute — was removed in the EdgionConfigData refactor; `overrideRef` is now a direct EdgionPlugins → EdgionConfigData reference.)

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

`WorkItemResult::Processed.persisted_status` is the write payload; never extract
it from the processed object in the Kubernetes worker. The object carries local
status for cache/conf_sync, while persistence preserves other deployments.
Replacing only typed status after handler parsing preserves compiled fields
and unknown-field sidecars; a full-object serde roundtrip here is incorrect.

Recognized native-only Kubernetes status is unowned input, never inherited by a
deployment. The first envelope write removes all retired native fields using a
fixed, kind-specific wire-only null set. Typed serialization may omit empty
arrays or nulls that still exist on the API object, so clearing only fields
visible after reserialization is insufficient. Logical comparison excludes these
tombstones. Mixed or unknown shapes suppress persistence without dropping config.

Native parent/ancestor arrays retain their API shape. Kubernetes projection
limits writes to attachments whose Gateway belongs to the selected class,
including when deployments share controllerName. Missing Gateway history is
preserved on disk but cannot become local serving authority. Delegation selects
one writer from the first sorted canonical parentRef before lookup; missing,
invalid or cyclic chains do not fall back to a visible alternative branch.
Serving resolution uses the owned raw Gateway and matching published UID/class,
never an unprojected raw Gateway fallback in Kubernetes.

Committed raw identity deltas for GatewayClass, Gateway and HTTP/GRPCRoute
enqueue dependents through their existing queues, including never-published
objects and complete-LIST removals. Status/resourceVersion echoes do not fan out.

The shared function sends `{metadata: {resourceVersion: raw_input_version}, status: ...}`.
Both `run_init_apply_once` and the runtime worker capture the raw source version before
processing; a processed sync version or a later GET's version must never replace it.
Versions are opaque strings. Missing or empty versions fail locally with `StatusWriteError`
and no API request; there is no unconditional-write fallback.

This applies automatically to all 17 status-computing kinds: the six parent-status resources,
BackendTLSPolicy, EdgionBackendTrafficPolicy, Gateway, GatewayClass, EdgionGatewayConfig,
EdgionPlugins, EdgionStreamPlugins, EdgionConfigData, EdgionBackend, LinkSys and EdgionAcme.
GatewayClass and EdgionGatewayConfig are cluster-scoped; the others are namespaced.
Future kinds using `ResourceController` inherit the guard without a per-kind switch.
Secret, ConfigMap, Service, EndpointSlice and ReferenceGrant do not compute status;
the separate Namespace observer does not write status either.

Exactly one PATCH is attempted per changed status. Any intervening successful object update
(including metadata/spec-only changes) makes the snapshot stale and yields 409 Conflict.
Non-404 failures use existing warning/error-metric paths; 404 remains an already-deleted
resource case. There is no immediate retry, GET, or status-failure requeue. A later relevant
event, successful source LIST, dependency requeue or leadership-acquisition requeue can
recompute from current raw state. Recovery has no time bound; a resumed WATCH alone does
not force reconciliation. Status IO remains inline in the single per-kind worker.

Version checking protects against stale writes, not incorrect snapshot ownership or stale
leadership. Kubernetes GatewayClass writes are restricted to the configured class and
controller-name pair. Gateway processing additionally requires that the referenced raw
class's nonempty UID matches this deployment's published class incarnation. Class
publication/unpublication requeues matching raw Gateways after the cache mutation;
ownership loss withdraws cached state rather than writing a rejection into another
controller's status. Generation and class `Accepted` do not define ownership. Independent
deployments must use distinct classes; replicas sharing one class/owner need the same
election domain and configuration view. This is not a fence for arbitrarily overlapping
independent writers. Handlers must preserve foreign `parents`/`ancestors` entries already in their
input, and explicitly serialize empty owned lists when clearing them. Both policy handlers
retain foreign ancestors while rebuilding only their own entries. ACME's two field-scoped
SSA service writers retain their separate responsibilities. See the
[snapshot concurrency guard](../../04-review/status/regression-guards/kubernetes-status-snapshot-concurrency.md).

For both BackendTLSPolicy and EdgionBackendTrafficPolicy, ancestor semantic diff uses the
shared `attachment_statuses_equal` one-to-one canonical identity plus controllerName comparison as parents,
including duplicate multiplicity. A first name/namespace-only match can compare an owned
entry against a foreign one and repeatedly report unchanged mixed-owner status as changed.
This shared semantic comparison applies across Kubernetes, FileSystem and etcd; the
resourceVersion PATCH precondition itself remains Kubernetes-specific.

### FileSystem mode
- `status_handler.write_status_value(kind, &key, &status_value)` → `.status` YAML file
- Guarded by: `status_changed` only (no leader check)

### Change detection — Semantic Status Diff

Condition messages must be deterministic for unchanged inputs. Sort names derived
from unordered collections before formatting diagnostics; do not remove `message`
from semantic comparison to suppress a feedback loop. Gateway conflict producers
follow this rule for both aggregate and per-listener messages; see the
[regression guard](../../04-review/status/regression-guards/deterministic-condition-messages.md).

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
| **Condition-only** | GatewayClass, EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, EdgionConfigData | `conditions_only_status_equal` (explicit arms or the condition-only fallback) | `status.conditions[]` |
| **Per-parent** | HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, EdgionTls | `attachment_statuses_equal` | Canonical parent identity plus controllerName; one-to-one semantic comparison with resource namespace context |
| **Per-ancestor** | BackendTLSPolicy, EdgionBackendTrafficPolicy | `attachment_statuses_equal` | `status.ancestors[]` matched one-to-one by canonical ancestorRef identity plus controllerName, including multiplicity |
| **Hybrid** | Gateway | `gateway_status_equal` | `addresses` + gateway-level `conditions` + `listeners[]` (matched by name) |
| **Hybrid (lifecycle + conditions)** | EdgionAcme, LinkSys | `conditions_and_fields_status_equal` | Semantic conditions plus structural comparison of all lifecycle fields |
| **K8s deployment observations** | EdgionPlugins, EdgionStreamPlugins, EdgionBackend, LinkSys | `controller_observations_equal` | Stable deployment key and class UID, then native payload semantics; ignores array order and nested condition transition times |
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
status is deployment-dependent on a shared resource, reuse `ControllerStatus<T>`
and register its native payload in the shared projection/merge, typed replacement,
CRD envelope and comparison inventory. Add alternating-deployment convergence,
foreign preservation and native FileSystem/etcd roundtrip tests. Only add a new
comparator when the structure does not fit an existing pattern.

#### Important edge cases

- `Empty → Present("{}")` is still treated as a change (triggers initial write)
- Invalid JSON on either side → always treated as changed (safe fallback)
- Condition arrays in different order → correctly handled (matched by `type`)
- Parent/listener arrays in different order → correctly handled (matched by identity)

## 8. Stale Status Cleanup

### Per-parent resources
Old parent statuses are cleaned up in two ways:

1. **Parent removed from spec**: `retain_current_parent_statuses()` removes
   owned parent statuses whose canonical `parent_ref` no longer matches a ref in the spec,
   and removes duplicate owned entries. Foreign entries from the input snapshot remain unchanged.
2. **`parent_refs` removed entirely (set to None)**: The `else` branch on the
   `if let Some(parent_refs)` removes only this controller's entries, using the same helper with an empty ref list.

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
- [ ] **parent_refs None → clear owned parents**: use the shared ownership-aware retention helper
- [ ] **Status struct serialization**: `skip_serializing_if` does not hide error information
- [ ] **No duplicate checks**: if validate() already checks something, update_status uses `validation_errors` instead of re-checking
- [ ] **No backend ref existence in validate()**: service-not-found and kind checks go in update_status only (via resolved_refs_errors + find_invalid_rule_indices), not in validate()
- [ ] **No "route" in non-route contexts**: use `resource_ns`, `set_parent_conditions_full`, etc.
- [ ] **No Ready condition** unless a real readiness signal exists; preserve Gateway/Gateway Listener Programmed semantics in `GatewayHandler`
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

### 2026-03-17: Ready removed; later Gateway Programmed behavior supersedes the old blanket rule
`Ready` remains absent because there is no resource-level data-plane acknowledgement.
The later Gateway implementation deliberately restored controller-derived
`Programmed` for Gateway and Gateway Listener status; use the current
`GatewayHandler` contract rather than the historical blanket removal.

## 11. Related

- [02-resource-controller.md](03-config-center/02-kubernetes/02-resource-controller.md) — Status persistence and leader guard
- [03-resource-system.md](../00-common/03-resource-system.md) — Resource system and preparse
- [00-guide.md](09-add-new-resource/00-guide.md) — Adding new resource types
- [11-conf-handler-guidelines.md](../02-gateway/11-conf-handler-guidelines.md) — ConfHandler (gateway side) guidelines
