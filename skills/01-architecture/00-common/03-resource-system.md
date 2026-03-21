---
name: resource-system
description: 资源系统架构：define_resources! 宏、ResourceMeta trait、ResourceKind 枚举、Preparse 机制、资源类型全表。
---

# 资源系统

## 唯一真相源 — `define_resources!`

所有资源在 `src/types/resource/defs.rs` 中通过 `define_resources!` 宏统一声明：

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
    // ... 所有其他类型
}
```

该宏自动生成：`ResourceKind` 枚举、`from_kind_name()` 方法、`from_content()` 方法、注册表元数据。

## ResourceMeta Trait

每个资源都通过 `impl_resource_meta!` 实现 `ResourceMeta`：

```rust
pub trait ResourceMeta {
    fn get_version(&self) -> u64;
    fn resource_kind() -> ResourceKind;
    fn kind_name() -> &'static str;
    fn key_name(&self) -> String;           // "namespace/name" 格式
    fn pre_parse(&mut self) { }             // 可选的预解析钩子
}
```

## ResourceKind 枚举

自动生成的枚举包含全部 20 种资源类型：

`GatewayClass`, `EdgionGatewayConfig`, `Gateway`, `HTTPRoute`, `GRPCRoute`, `TCPRoute`, `TLSRoute`, `UDPRoute`, `Service`, `EndpointSlice`, `Endpoint`, `Secret`, `EdgionTls`, `EdgionPlugins`, `EdgionStreamPlugins`, `PluginMetaData`, `LinkSys`, `ReferenceGrant`, `BackendTLSPolicy`, `EdgionAcme`

## 资源类型全表

| 分类 | 资源 | 集群级 | 同步到 Gateway | 说明 |
|------|------|:------:|:--------------:|------|
| **核心配置** | GatewayClass | ✓ | ✓ | Gateway API GatewayClass |
| | EdgionGatewayConfig | ✓ | ✓ | Edgion 全局配置 |
| | Gateway | ✗ | ✓ | Gateway API Gateway |
| **路由** | HTTPRoute | ✗ | ✓ | HTTP/HTTPS 路由规则 |
| | GRPCRoute | ✗ | ✓ | gRPC 路由规则 |
| | TCPRoute | ✗ | ✓ | TCP 路由规则 |
| | TLSRoute | ✗ | ✓ | TLS 路由规则 |
| | UDPRoute | ✗ | ✓ | UDP 路由规则 |
| **后端/服务** | Service | ✗ | ✓ | Kubernetes Service |
| | EndpointSlice | ✗ | ✓ | Kubernetes EndpointSlice |
| | Endpoint | ✗ | ✓ | Kubernetes Endpoint（旧版） |
| **安全/策略** | EdgionTls | ✗ | ✓ | TLS 证书配置 |
| | Secret | ✗ | **✗** | Kubernetes Secret（不同步） |
| | ReferenceGrant | ✗ | **✗** | 跨命名空间引用授权（不同步） |
| | BackendTLSPolicy | ✗ | ✓ | 后端 TLS 策略 |
| **插件/扩展** | EdgionPlugins | ✗ | ✓ | HTTP 层插件定义 |
| | EdgionStreamPlugins | ✗ | ✓ | Stream 层插件定义 |
| | PluginMetaData | ✗ | ✓ | 插件元数据 |
| **ACME** | EdgionAcme | ✗ | ✓ | ACME 自动证书 |
| **基础设施** | LinkSys | ✗ | ✓ | 外部系统连接器 |

> **注意**：Secret 和 ReferenceGrant 不同步到 Gateway，它们的数据在 Controller 侧被消费（Secret 内容嵌入到需要的资源中，ReferenceGrant 只在 Controller 做跨命名空间校验）。

## Preparse 机制

Preparse 在配置加载时（而非每请求时）构建运行时结构：

| 资源 | Preparse 用途 |
|------|-------------|
| HTTPRoute | 从 filters 构建 `PluginRuntime`，解析超时，解析 ExtensionRef LB |
| GRPCRoute | 同 HTTPRoute |
| EdgionPlugins | 验证所有插件配置，填充 `preparse_errors` |
| LinkSys | 验证端点、拓扑 |
| EdgionTls | 验证 TLS 配置 |

Preparse 在 **Controller**（用于状态报告）和 **Gateway**（用于构建运行时结构）**两侧都执行**。

## 相关文档

- [添加新资源指南](../../02-development/00-add-new-resource.md) — 添加新资源类型的步骤
- [配置中心](../01-controller/03-config-center/SKILL.md) — 资源如何经过 Workqueue 和 ResourceProcessor 处理
- [资源通用处理流程](../05-resources/00-resource-flow.md) — 从 Controller 到 Gateway 的完整流转
