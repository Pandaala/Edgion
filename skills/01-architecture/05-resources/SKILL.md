---
name: 05-resources
description: Resource taxonomy and processing — common processing flow, per-resource feature points / special handling / cross-resource relationships.
---

# 05 Resource Architecture

> This directory describes the **internal processing flow** of each resource from Controller to Gateway: per-stage Handler logic, requeue relationships, source code locations.
> The **external contract** of each resource (YAML schema, field types and defaults) lives in [02-features/03-resources/](../../02-features/03-resources/SKILL.md). Stable resource numbers are aligned where both pages exist; Controller-only and newly added resources are explicit exceptions.
>
> Each resource document follows a unified structure:
> 1. **Common-flow reference** — points to the common flow in [00-resource-flow.md](00-resource-flow.md)
> 2. **Resource-specific behavior** — concrete behavior of each Handler stage (filter/validate/preparse/parse/on_change/on_delete/update_status)
> 3. **Cross-resource relationships** — references, cascading updates, and requeue relationships between this resource and others

## File list

| File | Topic | Recommended reading scenario |
|------|------|-------------|
| [00-resource-flow.md](00-resource-flow.md) | Common resource processing flow | Understanding how resources flow from Controller to Gateway |
| [00-schema-transport-contract.md](00-schema-transport-contract.md) | CRD ↔ struct ↔ conf_sync ↔ redaction invariants | Adding or renaming any resource field |
| **Core configuration** | | |
| [01-gateway.md](01-gateway.md) | Gateway resource | Modifying Gateway/Listener logic |
| [02-gateway-class.md](02-gateway-class.md) | GatewayClass resource | Modifying GatewayClass handling |
| [03-edgion-gateway-config.md](03-edgion-gateway-config.md) | EdgionGatewayConfig resource | Modifying global configuration |
| **Routes** | | |
| [04-http-route.md](04-http-route.md) | HTTPRoute resource | Modifying HTTP routing |
| [05-grpc-route.md](05-grpc-route.md) | GRPCRoute resource | Modifying gRPC routing |
| [06-tcp-route.md](06-tcp-route.md) | TCPRoute resource | Modifying TCP routing |
| [07-tls-route.md](07-tls-route.md) | TLSRoute resource | Modifying TLS routing |
| [08-udp-route.md](08-udp-route.md) | UDPRoute resource | Modifying UDP routing |
| **Security and policy** | | |
| [09-edgion-tls.md](09-edgion-tls.md) | EdgionTls resource | Modifying TLS certificate management |
| [10-secret.md](10-secret.md) | Secret resource | Understanding Secret handling and security constraints |
| [11-reference-grant.md](11-reference-grant.md) | ReferenceGrant resource | Understanding cross-namespace references |
| [12-backend-tls-policy.md](12-backend-tls-policy.md) | BackendTLSPolicy resource | Configuring backend TLS |
| **Plugins and extensions** | | |
| [13-edgion-plugins.md](13-edgion-plugins.md) | EdgionPlugins resource | Understanding plugin configuration |
| [14-edgion-stream-plugins.md](14-edgion-stream-plugins.md) | EdgionStreamPlugins resource | Understanding Stream plugins |
| [15-edgion-config-data.md](15-edgion-config-data.md) | EdgionConfigData resource + overlay model | Understanding EdgionConfigData, base+overlay architecture, fail-to-base, variant taxonomy |
| **Backends and services** | | |
| [16-service-endpoints.md](16-service-endpoints.md) | Service + EndpointSlice | Backend discovery |
| **ACME** | | |
| [17-edgion-acme.md](17-edgion-acme.md) | EdgionAcme resource | Automatic certificates |
| **Infrastructure** | | |
| [18-link-sys.md](18-link-sys.md) | LinkSys resource | External system connections |
| [20-edgion-backend-traffic-policy.md](20-edgion-backend-traffic-policy.md) | EdgionBackendTrafficPolicy | Typed direct-backend policy for Service or EdgionBackend; never route-scoped |
| [21-configmap.md](21-configmap.md) | ConfigMap | Controller-side CA reference material |
| **AI Gateway** | | |
| [22-edgion-backend.md](22-edgion-backend.md) | EdgionBackend | Provider catalog, partial-good credential pool, route dependencies, and attempt runtime |
| **Relationships** | | |
| [19-resource-dependency.md](19-resource-dependency.md) | Inter-resource dependencies | Understanding cross-resource references, Store queries, and requeue cascades |

## Resource relationship overview

Spec-level reference relationships (simplified):

```
GatewayClass ──────► Gateway ◄────── EdgionGatewayConfig
                       │
            ┌──────────┼──────────┐
            ▼          ▼          ▼
       HTTPRoute   GRPCRoute   TCP/TLS/UDPRoute
            │          │          │
            ▼          ▼          ▼
         Service ──► EndpointSlice
            │
            ├────────► BackendTLSPolicy
            └────────► EdgionBackendTrafficPolicy

HTTPRoute ──► EdgionBackend ──► EdgionBackendTrafficPolicy

ConfigMap ──► BackendTLSPolicy (CA reference material)

Gateway ──► Secret (TLS certificates)
         ──► EdgionTls (extended TLS)
         ──► EdgionAcme (automatic certificates)

HTTPRoute/GRPCRoute ──► EdgionPlugins (HTTP plugins)
                    ──► EdgionStreamPlugins (Stream plugins)

EdgionPlugins/EdgionStreamPlugins ──► EdgionConfigData (overlay/data primitive via overrideRef)

EdgionBackend ──► Secret (bounded credential pool)
HTTPRoute.backendRefs ──► EdgionBackend (sole candidate/order/weight source)
EdgionPlugins/AiProxy ──► HTTPRoute candidates (selection algorithm only)

Cross-namespace references ──► ReferenceGrant

AccessLog/Plugin data ──► LinkSys (external systems)
```

Controller-internal Store queries and requeue cascades are documented in [19-resource-dependency.md](19-resource-dependency.md).
