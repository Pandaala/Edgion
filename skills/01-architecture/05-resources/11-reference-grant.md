---
name: resource-reference-grant
description: ReferenceGrant resource — cross-namespace reference authorization, not synced to Gateway, event-driven requeue.
---

# ReferenceGrant resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

ReferenceGrant is a Gateway API standard resource that authorizes cross-namespace resource references. It belongs to **no_sync_kind** and is not synced to the Gateway; it is maintained only in a global authorization store on the Controller side and, on change, triggers re-validation of affected resources via event dispatch.

## Source code locations

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant.rs`
- Authorization store: `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_grant/`
- Type definitions: `src/types/resources/reference_grant.rs`

## Not synced to Gateway

ReferenceGrant is included in `DEFAULT_NO_SYNC_KINDS` (`["ReferenceGrant", "Secret"]`) and is not synced to the Gateway via gRPC. Cross-namespace reference authorization decisions are made entirely on the Controller side, and the result is embedded as a `ref_denied` field on route resources synced to the Gateway.

## Controller-side processing

### parse

Write the ReferenceGrant into the global `ReferenceGrantStore` (upsert). The store provides a `check_reference_allowed()` method for other Handlers to check whether a cross-namespace reference is allowed.

### on_change

1. Collect the set of affected namespaces (including the ReferenceGrant's to namespace and all from namespaces)
2. Dispatch a change event via `ReferenceGrantChangedEvent`
3. The event listener (`CrossNsRefManager`), upon receiving the event, requeues all resources within affected namespaces that have cross-namespace references

### on_delete

1. Remove the entry from ReferenceGrantStore
2. Collect affected namespaces and dispatch a change event to trigger re-validation

## ReferenceGrant authorization check flow

When other resources' Handlers check cross-namespace references during the parse stage:

1. Detect that the namespace of the backendRef or secretRef differs from the resource's own namespace
2. Call `is_cross_ns_ref_allowed()` → `ReferenceGrantStore.check_reference_allowed()`
3. Check parameters: from_namespace, from_group, from_kind, to_namespace, to_group, to_kind, to_name
4. If not allowed: set the `ref_denied` field on the backendRef (the Gateway side rejects requests accordingly), or set a RefNotPermitted error on the Gateway TLS reference

ReferenceGrant validation can be disabled by configuration (`is_reference_grant_validation_enabled()`), in which case all cross-namespace references are allowed by default.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| ReferenceGrant → HTTPRoute | HTTPRoute | CrossNsRefManager + event dispatch | On change, requeue routes with cross-namespace backendRefs |
| ReferenceGrant → GRPCRoute | GRPCRoute | CrossNsRefManager + event dispatch | Same as above |
| ReferenceGrant → TCPRoute | TCPRoute | CrossNsRefManager + event dispatch | Same as above |
| ReferenceGrant → TLSRoute | TLSRoute | CrossNsRefManager + event dispatch | Same as above |
| ReferenceGrant → UDPRoute | UDPRoute | CrossNsRefManager + event dispatch | Same as above |
| ReferenceGrant → Gateway | Gateway | CrossNsRefManager + event dispatch | On change, requeue Gateways with cross-namespace certificateRefs |
