---
name: architecture
description: Use when understanding how Edgion works internally — Controller/Gateway/Center/Ctl architecture, gRPC sync, resource processing flow, or adding a new plugin/resource/connector.
---

# 01 Architecture and core features

> Edgion is a Kubernetes Gateway built on Pingora, using a Controller–Gateway split architecture.

## Overall architecture

Edgion is a single-Crate, four-binary project. **Controller decides, Gateway executes, Ctl operates, Center federates.**

```mermaid
flowchart TD
    User["User / K8s API"] --> Controller
    Traffic["Client traffic"] --> Gateway

    subgraph Controller["edgion-controller (control plane)"]
        CC["ConfCenter (File/K8s)"] --> WQ["Workqueue (per-kind)"]
        WQ --> RP["ResourceProcessor\n(validate/preparse/parse)"]
        RP --> SC["ServerCache"]
        CrossRef["Cross-resource dependencies\nSecretRefManager\nServiceRefManager\nCrossNamespaceRefManager\nRequeueChain"]
        CtrlServices["Admin API (:5800)\nConfigSyncServer (:50051)\nACME Service (optional, Leader only)"]
    end

    SC -- "gRPC Watch/List" --> SyncClient

    subgraph Gateway["edgion-gateway (data plane)"]
        SyncClient["ConfigSyncClient"] --> ClientCache["ClientCache"] --> ConfHandler["ConfHandler"]
        PingoraServer["Pingora Server"]
        ConnFilter["ConnectionFilter (StreamPlugins)"]
        ProxyHttp["ProxyHttp (HTTP/gRPC)\nroute match + plugins\nupstream_peer + LB\nlogging (AccessLog + Metrics)"]
        StreamRoutes["TCP/TLS/UDP Routes"]
        PingoraServer --- ConnFilter
        PingoraServer --- ProxyHttp
        PingoraServer --- StreamRoutes
        TLS["TLS subsystem\nDownstream TLS (client → gateway)\nUpstream TLS (gateway → backend mTLS)"]
        GwServices["Admin API (:5900)\nMetrics (:5901)"]
    end

    subgraph CLI["edgion-cli (CLI tool)"]
        CliOps["apply/delete/get/reload\n3 targets: center/server/client"]
    end

    subgraph Center["edgion-center (federation center)"]
        CenterNode["Aggregates resources across federated controllers\nServes Center API"]
    end

    CLI -- "HTTP API" --> Controller
```

## Fine-grained locator

| What you want to learn | Start here |
|-----------|-----------|
| Project at a glance, code organization, conventions shared by the four binaries | [00-common/](00-common/SKILL.md) |
| How the Controller processes resources internally | [01-controller/](01-controller/SKILL.md) |
| How the Gateway processes requests | [02-gateway/](02-gateway/SKILL.md) |
| How Controller and Gateway sync | [03-controller-gateway-link/](03-controller-gateway-link/SKILL.md) |
| edgion-cli commands and target modes | [04-ctl/](04-ctl/SKILL.md) |
| The full processing chain for a specific resource | [05-resources/](05-resources/SKILL.md) |
| How edgion-center federates multi-cluster Controllers | [06-center/](06-center/SKILL.md) |
| Troubleshoot route mismatch | [02-gateway/03-routes/](02-gateway/03-routes/SKILL.md) |
| Troubleshoot TLS certificate issues | [02-gateway/04-tls/](02-gateway/04-tls/SKILL.md) |
| Troubleshoot gRPC sync issues | [03-controller-gateway-link/00-overview.md](03-controller-gateway-link/00-overview.md) |
| **Develop a new resource type** | [01-controller/09-add-new-resource/](01-controller/09-add-new-resource/00-guide.md) |
| **Develop an HTTP plugin** | [02-gateway/12-edgion-plugin-dev.md](02-gateway/12-edgion-plugin-dev.md) |
| **Develop a Stream plugin** | [02-gateway/13-stream-plugin-dev.md](02-gateway/13-stream-plugin-dev.md) |
| **Develop a LinkSys connector** | [02-gateway/14-link-sys-dev.md](02-gateway/14-link-sys-dev.md) |
| **Implement a ConfHandler** | [02-gateway/11-conf-handler-guidelines.md](02-gateway/11-conf-handler-guidelines.md) |
| **Implement update_status** | [01-controller/10-status-management.md](01-controller/10-status-management.md) |

---

## Directory overview

### [00-common/](00-common/SKILL.md) — Common conventions

Foundational conventions shared by all four binaries: project overview, command line / directory / config-path conventions, Core module layering, and the resource system (`define_resources!` macro).

### [01-controller/](01-controller/SKILL.md) — edgion-controller architecture

Internal architecture of the control plane, broken down by module:

| Module | Keywords |
|------|--------|
| Overall architecture | ConfMgr facade, ProcessorRegistry |
| Startup / shutdown | Leader election, HA mode, event-driven main loop |
| Admin API | port :5800, CRUD, health/ready |
| ConfigCenter | ConfCenter trait, FileSystem / Kubernetes implementations |
| Workqueue | deduplication, exponential backoff, dirty requeue |
| ResourceProcessor | 11-step processing pipeline, 23 Handler kinds |
| Requeue | cross-resource linkage, TriggerChain cycle detection |
| CacheServer | ServerCache + EventStore, gRPC data source |
| ACME | Let's Encrypt automated certificates, Leader only |
| Adding a new resource | workflow + 4 template categories (route-like / controller-only / plugin-like / cluster-scoped) |
| Status management | update_status implementation conventions, known pitfalls |

### [02-gateway/](02-gateway/SKILL.md) — edgion-gateway architecture

Internal architecture of the data plane, broken down by subsystem:

| Subsystem | Keywords |
|--------|--------|
| Overall architecture | dual-runtime model (Tokio + Pingora) |
| Startup / shutdown | 14-step startup sequence, two-phase Pingora integration |
| Pingora lifecycle | 7-phase ProxyHttp callbacks, ConnectionFilter |
| [Routes](02-gateway/03-routes/) | multi-stage matching pipeline, separate engines for HTTP/gRPC/TCP/TLS/UDP |
| [TLS](02-gateway/04-tls/) | downstream/upstream TLS, SNI matching, BoringSSL/OpenSSL |
| Plugin system | 4-phase execution, 28 HTTP plugins, conditional execution |
| Load balancing | RoundRobin/EWMA/LeastConn/ConsistentHash/Weighted |
| Backends | Service/EndpointSlice discovery, health checks |
| Observability | AccessLog (zero-copy JSON), Prometheus Metrics |
| LinkSys | ES/Redis/Etcd/Webhook/File external integrations |
| Runtime store | Gateway/Route configuration, ArcSwap atomic swap |
| ConfHandler development | full_set/partial_update conventions, incremental update constraints |
| HTTP plugin development | EdgionPlugin phase selection, runtime wiring, ExtensionRef |
| Stream plugin development | ConnectionFilter / TlsRoute two phases |
| LinkSys development | external system connector development, LinkSysStore bridging |

### [03-controller-gateway-link/](03-controller-gateway-link/SKILL.md) — Bidirectional gRPC sync

The ConfigSync protocol (GetServerInfo / List / Watch / WatchServerMeta), the Controller-side ConfigSyncServer and the Gateway-side ConfigSyncClient implementation.

### [04-ctl/](04-ctl/SKILL.md) — edgion-cli architecture

CLI tool: 3 target modes (center/server/client), apply/delete/get/reload subcommands.

### [05-resources/](05-resources/SKILL.md) — Resource architecture

Common processing flow + features, special handling, and cross-resource links for all 20 resources.
For resource Schema (field definitions, default values) see [02-features/03-resources/](../02-features/03-resources/SKILL.md) (numbered to align, complementary responsibilities).

| Category | Resources |
|------|------|
| Core configuration | Gateway, GatewayClass, EdgionGatewayConfig |
| Routes | HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute |
| Security / policy | EdgionTls, Secret, ReferenceGrant, BackendTLSPolicy |
| Plugins / extensions | EdgionPlugins, EdgionStreamPlugins, PluginMetaData |
| Backends / services | Service, EndpointSlice, Endpoint |
| Automation | EdgionAcme |
| Infrastructure | LinkSys |

### [06-center/](06-center/SKILL.md) — edgion-center architecture

Federation center: accepts gRPC streams from multiple Controllers, reverse-Watch
aggregates PluginMetaData, persists controller registry to SQLite, and exposes an
HTTP Admin API.

| Module | Keywords |
|--------|---------|
| Overview | Deployment shape, ports, TLS posture, startup lifecycle, failure modes |
| Federation gRPC server | `RegisterRequest` validation caps, bidirectional stream protocol, error handling |
| Aggregator / watch cache | `ResourceAggregator`, `CenterWatchCache`, `CenterMetaDataStore`, reverse-Watch data flow |
| Persistence | SQLite `controllers` table, `CenterDb`, Admin API, Docker build notes |

### [06-gateway-api.md](06-gateway-api.md) — Gateway API conformance

Gateway API v1.5.0 supported scope, conformance tests, Edgion extension points, intentional deviations.
