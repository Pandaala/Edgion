---
name: controller-resource-processor
description: ResourceProcessor 流水线：ProcessorHandler trait、HandlerContext、11 步处理流程、23 种 Handler 实现、引用管理器。
---

# ResourceProcessor 流水线

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### ProcessorHandler trait

<!-- TODO:
```rust
pub trait ProcessorHandler<K>: Send + Sync {
    fn filter(&self, obj: &K) -> bool;
    fn clean_metadata(&self, obj: &mut K, ctx: &HandlerContext);
    fn validate(&self, obj: &K, ctx: &HandlerContext) -> Vec<String>;
    fn preparse(&self, obj: &mut K, ctx: &HandlerContext) -> Vec<String>;
    async fn parse(&self, obj: K, ctx: &HandlerContext) -> ProcessResult<K>;
    async fn on_apply(&self, obj: K, ctx: &HandlerContext) -> HandlerResult<K>;
    async fn on_delete(&self, key: &str, ctx: &HandlerContext) -> HandlerResult<()>;
    async fn on_init(&self, objs: Vec<K>, ctx: &HandlerContext) -> HandlerResult<Vec<K>>;
}
```
-->

### HandlerContext

<!-- TODO: secret_ref_manager, metadata_filter, namespace_filter, trigger_chain, requeue() -->

### 11 步处理流程

<!-- TODO: namespace filter → validate → preparse → parse → extract status → update status → check change → on_change → ServerCache 等 -->

### Handler 实现列表

<!-- TODO: 23 种 Handler，按分类列出 -->
<!-- Gateway API 路由：HttpRoute, GrpcRoute, TcpRoute, TlsRoute, UdpRoute -->
<!-- Gateway API 核心：Gateway, GatewayClass, BackendTlsPolicy, ReferenceGrant -->
<!-- K8s 核心：Service, Endpoints, EndpointSlice, Secret, Configmap -->
<!-- Edgion 自定义：EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, EdgionTls, EdgionAcme, PluginMetadata, LinkSys -->
<!-- 共享工具：route_utils, hostname_resolution -->

### 引用管理器

<!-- TODO:
- ReferenceGrantStore / CrossNamespaceValidator
- SecretRefManager / SecretStore
- ServiceRefManager
- ListenerPortManager
- AttachedRouteTracker
- GatewayRouteIndex
-->

### Status 工具

<!-- TODO: status_utils (Accepted, ResolvedRefs 条件)、status_diff (语义变更检测) -->
