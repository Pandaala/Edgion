---
name: resource-dependency
description: Quick reference for inter-resource dependencies — spec references, Controller-side Store queries, requeue cascades.
---

# Inter-resource dependencies

> Detailed analysis (planned, not yet created): `tasks/references/resource-dependency-map.md`

## Dependency relationship index

```mermaid
flowchart LR
    subgraph SecretDeps
        Secret -- "on_change requeue\nSecretRefManager registrants" --> Gateway
        Secret -- "certificateRef\nsecretRef / credentials" --> EdgionTls
        Secret --> EdgionPlugins
        Secret --> BackendTLSPolicy
        Secret --> EdgionAcme
        Secret --> EdgionBackend
    end

    subgraph ConfigMapDeps
        ConfigMap -- "on_change requeue\nSecretRefManager registrants" --> Gateway2[Gateway]
        ConfigMap -- "frontend CA\nbackend CA" --> BackendTLSPolicy2[BackendTLSPolicy]
    end

    subgraph ServiceDeps
        Service -- "on_change requeue\nServiceRefManager registrants" --> Routes["Route family\n(HTTP/GRPC/TCP/TLS/UDP)"]
        Service -- "RequestMirror target\nService / EndpointSlice requeue" --> MirrorPlugins[EdgionPlugins]
        Service -- "backendRef\nclusterIP" --> Gateway3["Gateway\n(derives status address)"]
    end

    subgraph ReferenceGrantDeps
        ReferenceGrant -- "on_change requeue\ncross-ns referrers" --> Gateway4["Gateway\n(cross-ns Secret/CA)"]
        ReferenceGrant -- "cross-ns gatewayRef" --> EdgionAcmeGrant[EdgionAcme]
        EdgionConfigData -- "ProxyProtocolTrust change/delete\nworkqueue requeue" --> Gateway4
        ReferenceGrant -- "cross-ns authorization check" --> Routes2["Route family\n(cross-ns backendRef)"]
        ReferenceGrant -- "cross-ns dns-resolver-ref" --> ServiceGrant[Service]
        ReferenceGrant -- "grant-gated ConfigMap CA" --> ConfigConsumers["EdgionGatewayConfig / EdgionPlugins"]
    end

    subgraph NamespaceDeps
        Namespace -- "label-change requeue\nSelector Gateway" --> Routes3["Route family\n(does the listener allow this namespace)"]
    end

    subgraph GatewayDeps
        GW[Gateway] -- "on_change requeue\ngateway_route_index" --> Routes4["Route family\n(verifies listener match)"]
        GW -- "parentRef" --> EdgionTls2[EdgionTls]
        GW -- "gatewayRef / proof epoch" --> EdgionAcme2[EdgionAcme]
        Routes4 -- "on_change requeue" --> GW
    end

    subgraph BackendDeps
        HTTPRouteBackend[HTTPRoute] -- "typed backendRef\nexistence / ReferenceGrant / variant" --> EdgionBackend
        EBTP[EdgionBackendTrafficPolicy] -- "typed same-namespace targetRef" --> EdgionBackend
        EdgionBackend -- "apply/delete requeue\nbackend_ref_mgr" --> HTTPRouteBackend
        EdgionBackend -- "apply/delete requeue\nbackend_ref_mgr" --> EBTP
        AiProxy[Rule-level EdgionPlugins / AiProxy] -- "selection algorithm only" --> HTTPRouteBackend
    end
```

## Stores: writers and readers

| Store | Writer | Reader |
|-------|--------|--------|
| **SecretStore** | SecretHandler | Gateway, EdgionTls, EdgionPlugins, BackendTLSPolicy, EdgionAcme, EdgionBackend |
| **ConfigMapStore** | ConfigMapHandler | Gateway (frontend CA), BackendTLSPolicy (backend CA), EdgionGatewayConfig and EdgionPlugins (grant-gated outbound CA) |
| **NamespaceStore** | K8s Namespace informer | Route family (allowedRoutes Selector) |
| **ReferenceGrantStore** | ReferenceGrantHandler | Gateway, Route family, Service, EdgionAcme, EdgionGatewayConfig, and EdgionPlugins (cross-ns authorization) |
| **SecretRefManager** | Gateway/EdgionTls/EdgionPlugins/BackendTLSPolicy/EdgionAcme/EdgionBackend (register during parse; ConfigMap material uses `ConfigMap/ns/name` keys to preserve kind) | SecretHandler/ConfigMapHandler (look up dependents on on_change using their kind-specific key) |
| **ServiceRefManager** | Route family (register forwarding and mirror targets during parse), EdgionPlugins (register enabled mirror targets during parse) | ServiceHandler and EndpointSliceHandler (look up dependents on apply/delete), BackendTLSPolicy (read forwarding-only keys in resolve_ancestor_gateways) |
| **CrossNsRefManager** | Gateway / Route family / Service / EdgionAcme / EdgionGatewayConfig / EdgionPlugins / EdgionStreamPlugins (clear-before-add and register cross-ns references during parse) | ReferenceGrantHandler (look up dependents on on_change) |
| **EdgionConfigDataRefManager** | Gateway and plugin resources (clear-before-add during parse) | EdgionConfigData handler (requeue dependents on apply/delete) |
| **ProxyProtocolListenerPolicyManager** | Gateway (atomic whole-Gateway contribution replacement) | Gateway handler (requeue only peers affected by a net physical-listener declaration change) |
| **GatewayRouteIndex** | Route family / EdgionTls / EdgionAcme (write on reconcile) | GatewayHandler (requeue route and attachment-proof consumers), BackendTLSPolicy (read in resolve_ancestor_gateways) |
| **RouteDelegationIndex** (`route_delegation_index`) | HTTPRoute (register delegation edges during parse: named children + wildcard namespaces, transitive) | HTTPRoute (on_change/on_delete: `parents_of()` requeues delegating parents; update_status: child-side `Accepted` for `kind: HTTPRoute` parentRefs) |
| **RouteDelegationIndex** (`grpc_route_delegation_index`, second instance, same type, disjoint `"ns/name"` keys) | GRPCRoute (register delegation edges during parse: named children + wildcard namespaces, transitive) | GRPCRoute (on_change/on_delete: `parents_of()` requeues delegating parents; update_status: child-side `Accepted` for `kind: GRPCRoute` parentRefs) |
| **BackendTLSPolicyTargetManager** | BackendTLSPolicy (register targetRef during parse) | BackendTLSPolicy (conflict detection in update_status) |
| **BackendRefManager** (`backend_ref_mgr`) | HTTPRoute, BackendTLSPolicy, and EdgionBackendTrafficPolicy (clear-before-add typed refs during parse) | EdgionBackend handler (requeue typed dependents on apply/delete); HTTPRoute recomputes current AI capability semantics |
| **PluginRefManager** | Gateway, HTTPRoute, and GRPCRoute (register rule- and backend-level EdgionPlugins ExtensionRefs during parse) | EdgionPlugins handler (requeue typed dependents on apply/delete) |
| **AiBackendStore** (Gateway ArcSwap runtime store) | EdgionBackend ConfigClient handler | AI attempt planner (single-snapshot typed backend lookup) |

`HTTPRoute.backendRefs` is the only typed EdgionBackend destination graph. Route reconcile validates
supported kind, object existence, cross-namespace authorization, and current variant semantics.
AI rules additionally enforce homogeneous AI forwarding and exactly one rule-level AiProxy.
Delegation carries terminal typed targets and
their resolved namespace. Plugins do not own a service reference or a second dependency graph.

An existing EdgionBackend satisfies the route's `ResolvedRefs` independently of the service's
credential status. The service reports partial-good or zero-good credential resolution on its own
conditions; the Gateway fails closed per request if the captured runtime snapshot has no usable
credential.

## Resources without external dependencies

ConfigMap, EndpointSlice, Namespace

EdgionConfigData is a provider for Gateway inbound PROXY protocol `resourceRef` policies and is
therefore no longer dependency-free. Provider changes and deletes requeue registered Gateways.
