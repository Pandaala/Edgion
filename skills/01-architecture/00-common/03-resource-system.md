---
name: resource-system
description: Resource system architecture — define_resources! macro, ResourceMeta trait, ResourceKind enum, Preparse mechanism, full resource type table.
---

# Resource system

## Single source of truth — `define_resources!`

All resources are declared centrally in `edgion-resources/src/resource/defs.rs` via the `define_resources!` macro:

```rust
define_resources! {
    Gateway => {
        kind_name: "Gateway",
        kind_aliases: &["gw"],
        cache_field: gateway_cache,
        capacity_field: gateway_capacity,
        default_capacity: 10,
        cluster_scoped: false,
        is_base_conf: false,
        in_registry: true,
    },
    // ... all other types
}
```

The macro auto-generates the `ResourceKind` enum, the `from_kind_name()` method, the `from_content()` method, and registry metadata.

## ResourceMeta trait

Every resource implements `ResourceMeta` via `impl_resource_meta!`:

```rust
pub trait ResourceMeta {
    fn get_version(&self) -> u64;
    fn resource_kind() -> ResourceKind;
    fn kind_name() -> &'static str;
    fn key_name(&self) -> String;           // "namespace/name" format
    fn pre_parse(&mut self) { }             // Optional pre-parse hook
}
```

## ResourceKind enum

The auto-generated enum contains all 20 resource types:

`GatewayClass`, `EdgionGatewayConfig`, `Gateway`, `HTTPRoute`, `GRPCRoute`, `TCPRoute`, `TLSRoute`, `UDPRoute`, `Service`, `EndpointSlice`, `Secret`, `EdgionTls`, `EdgionPlugins`, `EdgionStreamPlugins`, `PluginMetaData`, `LinkSys`, `ReferenceGrant`, `BackendTLSPolicy`, `EdgionAcme`, `ConfigMap`

## Full resource type table

| Category | Resource | Cluster-scoped | Synced to Gateway | Description |
|------|------|:------:|:--------------:|------|
| **Core configuration** | GatewayClass | ✓ | ✓ | Gateway API GatewayClass |
| | EdgionGatewayConfig | ✓ | ✓ | Edgion global configuration |
| | Gateway | ✗ | ✓ | Gateway API Gateway |
| **Routes** | HTTPRoute | ✗ | ✓ | HTTP/HTTPS routing rules |
| | GRPCRoute | ✗ | ✓ | gRPC routing rules |
| | TCPRoute | ✗ | ✓ | TCP routing rules |
| | TLSRoute | ✗ | ✓ | TLS routing rules |
| | UDPRoute | ✗ | ✓ | UDP routing rules |
| **Backends / services** | Service | ✗ | ✓ | Kubernetes Service |
| | EndpointSlice | ✗ | ✓ | Kubernetes EndpointSlice |
| **Security / policy** | EdgionTls | ✗ | ✓ | TLS certificate configuration |
| | Secret | ✗ | **✗** | Kubernetes Secret (not synced) |
| | ReferenceGrant | ✗ | **✗** | Cross-namespace reference authorization (not synced) |
| | BackendTLSPolicy | ✗ | ✓ | Backend TLS policy |
| **Plugins / extensions** | EdgionPlugins | ✗ | ✓ | HTTP-layer plugin definitions |
| | EdgionStreamPlugins | ✗ | ✓ | Stream-layer plugin definitions |
| | PluginMetaData | ✗ | ✓ | Plugin metadata |
| **ACME** | EdgionAcme | ✗ | ✓ | ACME automated certificates |
| **Infrastructure** | LinkSys | ✗ | ✓ | External system connectors |
| | ConfigMap | ✗ | ✗ | Kubernetes ConfigMap (not synced to Gateway) |

> **Note**: Secret and ReferenceGrant are not synced to the Gateway; their data is consumed on the Controller side (Secret content is embedded into the resources that need it; ReferenceGrant is used only on the Controller for cross-namespace validation).

## Preparse mechanism

Preparse builds runtime structures at config-load time (rather than per request):

| Resource | Preparse purpose |
|------|-------------|
| HTTPRoute | Build `PluginRuntime` from filters, parse timeouts, resolve ExtensionRef LB |
| GRPCRoute | Same as HTTPRoute |
| EdgionPlugins | Validate all plugin configurations, populate `preparse_errors` |
| LinkSys | Validate endpoints and topology |
| EdgionTls | Validate TLS configuration |

Preparse runs on **both the Controller** (for status reporting) **and the Gateway** (for building runtime structures).

## Related documentation

- [Add-new-resource guide](../01-controller/09-add-new-resource/00-guide.md) — Steps to add a new resource type
- [ConfigCenter](../01-controller/03-config-center/SKILL.md) — How resources flow through Workqueue and ResourceProcessor
- [Resource common processing flow](../05-resources/00-resource-flow.md) — End-to-end flow from Controller to Gateway
