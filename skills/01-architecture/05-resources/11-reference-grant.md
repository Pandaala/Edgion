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

The raw map and namespace index share one lock. Each indexed namespace bucket
retains the exact raw grant key, so upsert/remove touch only the previous and new
namespace buckets (`ARCH-110`); changing namespace metadata also removes the old
slot. Absent namespace metadata remains raw-only. `replace_all` builds the full
index and publishes both maps together. This avoids unrelated raw-map traversal
per grant mutation at the cost of one additional owned key and hash-table entry
per namespaced grant, without duplicating resource bodies. Lookup order remains
unspecified; authorization revisions retain their sorted UID/resourceVersion
representation. No measured latency or memory improvement is claimed.

### on_change

1. The affected namespace is the ReferenceGrant's own (to) namespace only. `CrossNsRefManager` keys every dependency by the referenced target namespace, so owners in both old and new `from` namespaces are already registered there; dispatching `from` namespaces would requeue unrelated resources that reference into those namespaces (on a busy controller this fan-out delayed the actual dependents by many seconds)
2. Dispatch a change event via `ReferenceGrantChangedEvent`
3. The event listener (`CrossNsRevalidationListener`), upon receiving the event, requeues every resource registered under the affected namespace in `CrossNsRefManager`

### on_delete

1. Remove the entry from ReferenceGrantStore
2. Collect affected namespaces and dispatch a change event to trigger re-validation

## ReferenceGrant authorization check flow

When other resources' Handlers check cross-namespace references during the parse stage:

1. Detect that the namespace of the backendRef or secretRef differs from the resource's own namespace
2. Register the owner under the target namespace before reading authorization, so a concurrent grant change cannot miss the dependency
3. Pass the actual owning field's canonical `GrantQuery` to `ReferenceGrantStore::check_query()` (bound material acquisition uses `query_authorization_revision()` to retain the same authorization snapshot)
4. Check parameters: from_namespace, from_group, from_kind, to_namespace, to_group, to_kind, to_name
5. If not allowed: set the `ref_denied` field on the backendRef (HTTP/gRPC return 500; TCP/UDP/TLS fail the connection closed before Service lookup), or set a RefNotPermitted error on the Gateway TLS reference

ReferenceGrant validation configuration (`is_reference_grant_validation_enabled()`) affects explicit field policies, not every cross-namespace reference. Gateway and EdgionTls have finite bypass policies inside bound material acquisition; this is not an exhaustive list of flag consumers. Route/backend/mirror, ConfigData and DNS retain their existing policies. BackendTLSPolicy CA and client-certificate targets are strictly policy-local: cross-namespace targets are rejected regardless of grants, and same-namespace targets need no grant. Disabling validation never relaxes target syntax or removes required recovery dependencies.

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
| ReferenceGrant → EdgionPlugins | EdgionPlugins | CrossNsRefManager + event dispatch | Authorizes grant-gated cross-namespace Secret/ConfigMap TLS references. Cluster-scoped EdgionGatewayConfig instead uses exclusive administrative field selection with an explicit target namespace; it does not fabricate an empty source namespace or use ReferenceGrant authority. |
| ReferenceGrant → Gateway inbound PROXY policy | Gateway | EdgionConfigDataRefManager + CrossNsRefManager + workqueue requeue | For a cross-namespace `ProxyProtocolTrust` target with `visibility: Namespace`, require `from {group: gateway.networking.k8s.io, kind: Gateway}` to `to {group: edgion.io, kind: EdgionConfigData}`. Cluster-visible and same-namespace targets need no grant. Denial is transported as explicit Invalid and fails Gateway startup closed. |
| ReferenceGrant → EdgionPlugins | EdgionPlugins | CrossNsRefManager + event dispatch | On change, requeue EdgionPlugins with cross-namespace EdgionConfigData refs (`allowRefs`/`denyRefs`, `clusterRegionRef`, selectors, and RequestAccessPolicy IP/URL data refs). Optional RequestAccessPolicy IP/URL denials are `PartiallyInvalid` and omit only that data source. Grant shape: `from {group: edgion.io, kind: EdgionPlugins}`, `to {group: edgion.io, kind: EdgionConfigData}`. |
| ReferenceGrant → EdgionStreamPlugins | EdgionStreamPlugins | CrossNsRefManager + event dispatch | On change, requeue EdgionStreamPlugins with cross-namespace EdgionConfigData refs (`allowRefs`/`denyRefs`, `activeProfileRef`). Grant shape: `from {group: edgion.io, kind: EdgionStreamPlugins}`, `to {group: edgion.io, kind: EdgionConfigData}`. |

## EdgionConfigData `spec.visibility` interaction

`ReferenceGrant` enforcement for EdgionConfigData refs is complemented by the target's
`spec.visibility` field:

- `Namespace` (default): cross-ns refs require a matching `ReferenceGrant` in the target namespace.
- `Cluster`: any namespace may reference without a matching grant. Some readers still consult the Grant store before applying this visibility rule; permission does not depend on finding a match.
- Same-namespace refs are always allowed regardless of `visibility`.

See [15-edgion-config-data.md — Cross-Namespace Reference Grants](15-edgion-config-data.md#cross-namespace-reference-grants) for the full mechanism, data-plane fail semantics, and status reason strings.

### Live IpList read proofs

An absent `refDenied` is not a positive authorization proof. Controller-owned IP
slot metadata distinguishes intrinsic SameNamespace, current-visibility ClusterOnly,
GrantAuthorized, ValidationDisabled and Denied. SameNamespace requires actual namespace
equality; the three cross-namespace scopes require an actual cross-namespace target.
Missing cross-namespace targets default to ClusterOnly until positive Grant processing;
a later private target cannot become readable under a stale absence marker. ClusterOnly
checks current raw visibility from the same snapshot as membership. Existing Grant
revocation/requeue latency remains; this is not an atomic cross-store Grant transaction.
ValidationDisabled changes permission policy only, never identity/type/enable checks.
