---
name: 03-resources
description: Feature and configuration schema reference for all Kubernetes resources. Numbering aligns with 01-architecture/05-resources, organized uniformly by resource type.
---

# 03 Resource Feature Reference

> **External contracts** for all Gateway API standard resources and Edgion extension resources: full YAML schema, field types and default values, configuration examples.
> The **internal implementation** of resources (Handler flow, requeue associations, source locations) is in [01-architecture/05-resources/](../../01-architecture/05-resources/SKILL.md). Stable resource numbers align where both pages exist; Controller-only ConfigMap and other explicit exceptions are listed by the architecture index.

## Numbering Alignment

| # | Resource | Architecture Doc | Feature Doc (this directory) |
|---|----------|------------------|------------------------------|
| 00 | Common concepts | `05-resources/00-resource-flow.md` | [00-common-concepts.md](00-common-concepts.md) |
| 01 | Gateway | `05-resources/01-gateway.md` | [01-gateway.md](01-gateway.md) |
| 02 | GatewayClass | `05-resources/02-gateway-class.md` | [02-gateway-class.md](02-gateway-class.md) |
| 03 | EdgionGatewayConfig | `05-resources/03-edgion-gateway-config.md` | → [../02-config/02-edgion-gateway-config.md](../02-config/02-edgion-gateway-config.md) |
| 04 | HTTPRoute | `05-resources/04-http-route.md` | [04-httproute.md](04-httproute.md) |
| 05 | GRPCRoute | `05-resources/05-grpc-route.md` | [05-grpcroute.md](05-grpcroute.md) |
| 06 | TCPRoute | `05-resources/06-tcp-route.md` | [06-tcproute.md](06-tcproute.md) |
| 07 | TLSRoute | `05-resources/07-tls-route.md` | [07-tlsroute.md](07-tlsroute.md) |
| 08 | UDPRoute | `05-resources/08-udp-route.md` | [08-udproute.md](08-udproute.md) |
| 09 | EdgionTls | `05-resources/09-edgion-tls.md` | [09-edgion-tls.md](09-edgion-tls.md) |
| 12 | BackendTLSPolicy | `05-resources/12-backend-tls-policy.md` | [12-backend-tls-policy.md](12-backend-tls-policy.md) |
| 13 | EdgionPlugins | `05-resources/13-edgion-plugins.md` | [13-edgion-plugins.md](13-edgion-plugins.md) |
| 14 | EdgionStreamPlugins | `05-resources/14-edgion-stream-plugins.md` | [14-edgion-stream-plugins.md](14-edgion-stream-plugins.md) |
| 15 | EdgionConfigData | `05-resources/15-edgion-config-data.md` | Plugin metadata, including the RegionRoute state-management variant |
| 16 | Service/EndpointSlice | `05-resources/16-service-endpoints.md` | [16-service-backends.md](16-service-backends.md) |
| 17 | EdgionAcme | `05-resources/17-edgion-acme.md` | [17-acme.md](17-acme.md) |
| 18 | LinkSys | `05-resources/18-link-sys.md` | [18-link-sys.md](18-link-sys.md) |
| 20 | EdgionBackendTrafficPolicy | `05-resources/20-edgion-backend-traffic-policy.md` | [20-edgion-backend-traffic-policy.md](20-edgion-backend-traffic-policy.md) |
| 22 | EdgionBackend | `05-resources/22-edgion-backend.md` | [22-edgion-backend.md](22-edgion-backend.md) |

> The feature documentation for #03 (EdgionGatewayConfig) lives under `02-config/` because it is essentially a configuration-layer resource.
> #10 (Secret), #11 (ReferenceGrant), and #15 (EdgionConfigData) currently have no standalone feature documentation; see the architecture documentation.
> ConfigMap is Controller-only and is documented at
> [01-architecture/05-resources/21-configmap.md](../../01-architecture/05-resources/21-configmap.md).

## Browse by Category

### Gateway Entry

| Resource | Description |
|----------|-------------|
| [01-gateway.md](01-gateway.md) | Listener configuration, protocols, ports, TLS binding, AllowedRoutes |
| [02-gateway-class.md](02-gateway-class.md) | GatewayClass and parametersRef |

### Routes

| Resource | Description |
|----------|-------------|
| [00-common-concepts.md](00-common-concepts.md) | Common parentRef/backendRef concepts, cross-namespace, hostname intersection |
| [04-httproute.md](04-httproute.md) | HTTP route matching, Filters, timeouts and retries, Session Persistence |
| [05-grpcroute.md](05-grpcroute.md) | gRPC route matching, gRPC-Web |
| [06-tcproute.md](06-tcproute.md) | TCP routing, Stream plugin annotations |
| [07-tlsroute.md](07-tlsroute.md) | TLS Passthrough routing, SNI matching |
| [08-udproute.md](08-udproute.md) | UDP routing |

### TLS and Certificates

| Resource | Description |
|----------|-------------|
| [09-edgion-tls.md](09-edgion-tls.md) | Extended TLS: mTLS, versions, cipher suites, OCSP |
| [12-backend-tls-policy.md](12-backend-tls-policy.md) | Upstream TLS validation, CA, SAN |
| [17-acme.md](17-acme.md) | ACME automatic certificates: HTTP-01, renewal, storage |

### Plugins

| Resource | Description |
|----------|-------------|
| [13-edgion-plugins.md](13-edgion-plugins.md) | HTTP plugin catalog; `EdgionPlugin` enum is authoritative |
| [14-edgion-stream-plugins.md](14-edgion-stream-plugins.md) | Stream plugin two-stage model |

### Backends and External Systems

| Resource | Description |
|----------|-------------|
| [16-service-backends.md](16-service-backends.md) | Service discovery |
| [20-edgion-backend-traffic-policy.md](20-edgion-backend-traffic-policy.md) | Direct-backend policy for Service or EdgionBackend; target kind controls valid sections |
| [18-link-sys.md](18-link-sys.md) | Redis/Etcd/ES/Webhook/Kafka/HttpDns connector and Controller-managed credential-source configuration |
### AI Gateway

| Resource | Description |
|----------|-------------|
| [22-edgion-backend.md](22-edgion-backend.md) | AI provider backends, HTTPRoute binding, status, and policy attachment |

## Quick Lookup

| You want to… | Open directly |
|--------------|---------------|
| Configure a Gateway Listener | [01-gateway.md](01-gateway.md) |
| Configure HTTP routing rules | [04-httproute.md](04-httproute.md) |
| Configure gRPC routes | [05-grpcroute.md](05-grpcroute.md) |
| Configure TLS/mTLS | [09-edgion-tls.md](09-edgion-tls.md) |
| Configure automatic certificates | [17-acme.md](17-acme.md) |
| Look up HTTP plugins | [13-edgion-plugins.md](13-edgion-plugins.md) |
| Configure TCP-layer plugins | [14-edgion-stream-plugins.md](14-edgion-stream-plugins.md) |
| Configure Service discovery | [16-service-backends.md](16-service-backends.md) |
| Configure health checks/circuit breaking/load balancing | [20-edgion-backend-traffic-policy.md](20-edgion-backend-traffic-policy.md) |
| Integrate with external systems | [18-link-sys.md](18-link-sys.md) |
| Configure upstream TLS | [12-backend-tls-policy.md](12-backend-tls-policy.md) |
| Configure an AI provider backend | [22-edgion-backend.md](22-edgion-backend.md) |
