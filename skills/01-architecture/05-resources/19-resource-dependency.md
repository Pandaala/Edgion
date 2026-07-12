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
    end

    subgraph ConfigMapDeps
        ConfigMap -- "on_change requeue\nSecretRefManager registrants" --> Gateway2[Gateway]
        ConfigMap -- "frontend CA\nbackend CA" --> BackendTLSPolicy2[BackendTLSPolicy]
    end

    subgraph ServiceDeps
        Service -- "on_change requeue\nServiceRefManager registrants" --> Routes["Route family\n(HTTP/GRPC/TCP/TLS/UDP)"]
        Service -- "backendRef\nclusterIP" --> Gateway3["Gateway\n(derives status address)"]
    end

    subgraph ReferenceGrantDeps
        ReferenceGrant -- "on_change requeue\ncross-ns referrers" --> Gateway4["Gateway\n(cross-ns Secret/CA)"]
        ReferenceGrant -- "cross-ns authorization check" --> Routes2["Route family\n(cross-ns backendRef)"]
    end

    subgraph NamespaceDeps
        Namespace -- "label-change requeue\nSelector Gateway" --> Routes3["Route family\n(does the listener allow this namespace)"]
    end

    subgraph GatewayDeps
        GW[Gateway] -- "on_change requeue\ngateway_route_index" --> Routes4["Route family\n(verifies listener match)"]
        GW -- "parentRef" --> EdgionTls2[EdgionTls]
        Routes4 -- "on_change requeue" --> GW
    end
```

## Stores: writers and readers

| Store | Writer | Reader |
|-------|--------|--------|
| **SecretStore** | SecretHandler | Gateway, EdgionTls, EdgionPlugins, BackendTLSPolicy, EdgionAcme |
| **ConfigMapStore** | ConfigMapHandler | Gateway (frontend CA), BackendTLSPolicy (backend CA) |
| **NamespaceStore** | K8s Namespace informer | Route family (allowedRoutes Selector) |
| **ReferenceGrantStore** | ReferenceGrantHandler | Gateway, Route family (cross-ns authorization) |
| **SecretRefManager** | Gateway/EdgionTls/EdgionPlugins/BackendTLSPolicy/EdgionAcme (register during parse) | SecretHandler/ConfigMapHandler (look up dependents on on_change) |
| **ServiceRefManager** | Route family (register backends during parse) | ServiceHandler (look up dependents on on_change), BackendTLSPolicy (read in resolve_ancestor_gateways) |
| **CrossNsRefManager** | Gateway / Route family (register cross-ns references during parse) | ReferenceGrantHandler (look up dependents on on_change) |
| **GatewayRouteIndex** | Route family / EdgionTls (write on on_change) | GatewayHandler (look up associated Routes on on_change), BackendTLSPolicy (read in resolve_ancestor_gateways) |
| **RouteDelegationIndex** (`route_delegation_index`) | HTTPRoute (register delegation edges during parse: named children + wildcard namespaces, transitive) | HTTPRoute (on_change/on_delete: `parents_of()` requeues delegating parents; update_status: child-side `Accepted` for `kind: HTTPRoute` parentRefs) |
| **RouteDelegationIndex** (`grpc_route_delegation_index`, second instance, same type, disjoint `"ns/name"` keys) | GRPCRoute (register delegation edges during parse: named children + wildcard namespaces, transitive) | GRPCRoute (on_change/on_delete: `parents_of()` requeues delegating parents; update_status: child-side `Accepted` for `kind: GRPCRoute` parentRefs) |
| **BackendTLSPolicyTargetManager** | BackendTLSPolicy (register targetRef during parse) | BackendTLSPolicy (conflict detection in update_status) |

## Resources without external dependencies

EdgionGatewayConfig, EdgionStreamPlugins, LinkSys, EdgionConfigData, Endpoints/EndpointSlice, Namespace
