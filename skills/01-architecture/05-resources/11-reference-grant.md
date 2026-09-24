---
name: resource-reference-grant
description: ReferenceGrant resource — cross-namespace reference authorization, not synced to Gateway, event-driven requeue.
---

# ReferenceGrant resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

ReferenceGrant is a Gateway API standard resource that authorizes cross-namespace resource references. It belongs to **no_sync_kind** and is not synced to the Gateway; it is maintained only in a global authorization store on the Controller side and, on change, triggers re-validation of affected resources via event dispatch.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant.rs`
- Authorization store: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/ref_grant/`
- Type definitions: `edgion-resources/src/resources/reference_grant.rs`

## Source versions

The canonical Controller type is ReferenceGrant v1. Kubernetes selects one
transport version per center lifetime: v1 if served, otherwise v1beta1 only
when v1 is absent. The v1beta1 watcher converts each event before the raw
store and the single ReferenceGrant processor. FileSystem, etcd, and Admin
input convert v1beta1 before typed processing. Kubernetes Admin writes preserve the authored version through source conversion,
then submit through the selected transport. Direct Kubernetes API clients still
require their submitted version to be served by the cluster.
The beta Rust root is named
`ReferenceGrantBeta`, but its wire kind is `ReferenceGrant`. ReferenceGrant
has no native status subresource; the Controller does not issue status writes.

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
2. Register the owner under the target namespace before reading authorization, so a concurrent grant change cannot miss the dependency
3. Call `is_cross_ns_ref_allowed()` → `ReferenceGrantStore.check_reference_allowed()`
4. Check parameters: from_namespace, from_group, from_kind, to_namespace, to_group, to_kind, to_name
5. If not allowed: set the `ref_denied` field on the backendRef (HTTP/gRPC return 500; TCP/UDP/TLS fail the connection closed before Service lookup), or set a RefNotPermitted error on the Gateway TLS reference

ReferenceGrant validation can be disabled by configuration (`is_reference_grant_validation_enabled()`), in which case all cross-namespace references are allowed by default.

RequestMirror backend references use the same `RefDenied` transport contract.
Native HTTPRoute filters authorize with the declaring route's identity, including
delegated rule/backend filters; EdgionPlugins mirrors authorize as
`edgion.io/EdgionPlugins`. Denial reports `ResolvedRefs=False/RefNotPermitted` on
the owning resource and skips only the mirror before endpoint selection, leaving
the primary request active. Both publishers register cross-namespace dependencies
for grant-change revalidation. See the
[mirror authorization guard](../../04-review/request-mirror/regression-guards/mirror-reference-grant-enforcement.md).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| ReferenceGrant → HTTPRoute | HTTPRoute | CrossNsRefManager + event dispatch | On change, requeue routes with cross-namespace backendRefs |
| ReferenceGrant → GRPCRoute | GRPCRoute | CrossNsRefManager + event dispatch | Same as above |
| ReferenceGrant → TCPRoute | TCPRoute | CrossNsRefManager + event dispatch | Same as above |
| ReferenceGrant → TLSRoute | TLSRoute | CrossNsRefManager + event dispatch | Same as above |
| ReferenceGrant → UDPRoute | UDPRoute | CrossNsRefManager + event dispatch | Same as above |
| ReferenceGrant → Gateway | Gateway | CrossNsRefManager + event dispatch | On change, requeue Gateways with cross-namespace certificateRefs |
| ReferenceGrant → Service | Service | CrossNsRefManager + event dispatch | Authorizes cross-namespace `edgion.io/dns-resolver-ref` targets and requeues the Service when grants change |
| ReferenceGrant → EdgionAcme | EdgionAcme | CrossNsRefManager + event dispatch | Authorizes `edgion.io/EdgionAcme` to select a cross-namespace `gateway.networking.k8s.io/Gateway`; changes re-resolve HTTP-01 serving attachments |
| ReferenceGrant → EdgionGatewayConfig / EdgionPlugins | EdgionGatewayConfig, EdgionPlugins | CrossNsRefManager + event dispatch | Authorizes grant-gated cross-namespace ConfigMap CA references. Cluster-scoped EdgionGatewayConfig uses an empty declaring namespace and requires an explicit target namespace. |
| ReferenceGrant → Gateway inbound PROXY policy | Gateway | EdgionConfigDataRefManager + CrossNsRefManager + workqueue requeue | For a cross-namespace `ProxyProtocolTrust` target with `visibility: Namespace`, require `from {group: gateway.networking.k8s.io, kind: Gateway}` to `to {group: edgion.io, kind: EdgionConfigData}`. Cluster-visible and same-namespace targets need no grant. Denial is transported as explicit Invalid and fails Gateway startup closed. |
| ReferenceGrant → EdgionPlugins | EdgionPlugins | CrossNsRefManager + event dispatch | On change, requeue EdgionPlugins with cross-namespace EdgionConfigData refs (`allowRefs`/`denyRefs`, `overrideRef`, selectors, and RequestAccessPolicy IP/URL data refs). Optional RequestAccessPolicy IP/URL denials are `PartiallyInvalid` and omit only that data source. Grant shape: `from {group: edgion.io, kind: EdgionPlugins}`, `to {group: edgion.io, kind: EdgionConfigData}`. |
| ReferenceGrant → EdgionStreamPlugins | EdgionStreamPlugins | CrossNsRefManager + event dispatch | On change, requeue EdgionStreamPlugins with cross-namespace EdgionConfigData refs (`allowRefs`/`denyRefs`, `activeProfileRef`). Grant shape: `from {group: edgion.io, kind: EdgionStreamPlugins}`, `to {group: edgion.io, kind: EdgionConfigData}`. |

## EdgionConfigData `spec.visibility` interaction

`ReferenceGrant` enforcement for EdgionConfigData refs is complemented by the target's
`spec.visibility` field:

- `Namespace` (default): cross-ns refs require a matching `ReferenceGrant` in the target namespace.
- `Cluster`: any namespace may reference without a grant (no grant is consulted).
- Same-namespace refs are always allowed regardless of `visibility`.

See [15-edgion-config-data.md — Cross-Namespace Reference Grants](15-edgion-config-data.md#cross-namespace-reference-grants) for the full mechanism, data-plane fail semantics, and status reason strings.
