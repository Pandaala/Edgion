# 资源系统

> Edgion 的资源抽象：`define_resources!` 宏统一声明、`ResourceMeta` trait、`ResourceKind` 枚举、Preparse 机制。

## Single Source of Truth — `define_resources!`

All resources are declared once in `src/types/resource/defs.rs` via the `define_resources!` macro:

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
    // ... all other kinds
}
```

This generates: `ResourceKind` enum, `from_kind_name()`, `from_content()`, registry metadata.

## ResourceMeta Trait

Every resource implements `ResourceMeta` (via `impl_resource_meta!`):

```rust
pub trait ResourceMeta {
    fn get_version(&self) -> u64;
    fn resource_kind() -> ResourceKind;
    fn kind_name() -> &'static str;
    fn key_name(&self) -> String;           // "namespace/name"
    fn pre_parse(&mut self) { }             // Optional preparse hook
}
```

## ResourceKind Enum

`GatewayClass`, `EdgionGatewayConfig`, `Gateway`, `HTTPRoute`, `GRPCRoute`, `TCPRoute`, `TLSRoute`, `UDPRoute`, `Service`, `EndpointSlice`, `Endpoint`, `Secret`, `EdgionTls`, `EdgionPlugins`, `EdgionStreamPlugins`, `PluginMetaData`, `LinkSys`, `ReferenceGrant`, `BackendTLSPolicy`, `EdgionAcme`

## Resource Preparse

Preparse builds runtime-ready structures at config-load time (not per-request):

| Resource | Preparse Purpose |
|----------|-----------------|
| `HTTPRoute` | Build `PluginRuntime` from filters, parse timeouts, resolve `ExtensionRef` LB |
| `GRPCRoute` | Same as HTTPRoute (hidden logic, timeouts) |
| `EdgionPlugins` | Validate all plugin configs, fill `preparse_errors` |
| `LinkSys` | Validate endpoints, topology |
| `EdgionTls` | Validate TLS config |

Preparse runs in **both** controller (for status reporting) and gateway (for runtime structures).

## Related

- [Add New Resource Guide](../development/00-add-new-resource.md) — step-by-step guide for adding new resource types
- [Config Center](01-config-center/SKILL.md) — how resources are processed through Workqueue and ResourceProcessor
