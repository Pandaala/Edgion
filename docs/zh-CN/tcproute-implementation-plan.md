# TCPRoute Implementation Plan

本文档详细说明如何在 Edgion 项目中添加 TCPRoute 支持，完全遵循 GRPCRoute 的实现模式。

## 概述

TCPRoute 是 Kubernetes Gateway API 的一部分，用于路由 TCP 流量。与 HTTPRoute/GRPCRoute 相比：

- **无 hostname 匹配**: TCP 层面没有 Host 信息
- **简化的匹配规则**: 主要基于端口和 SNI（TLS）
- **不同的过滤器**: 没有 HTTP 层面的 header 修改等操作
- **API 版本**: `gateway.networking.k8s.io/v1alpha2` (注意：TCPRoute 目前仍在 alpha 阶段)

## 架构流程

TCPRoute 将遵循与 GRPCRoute 相同的架构：

```
TCPRoute YAML → Resource Type → ResourceMeta → ResourceKind 
→ Proto → ConfigServer/ConfigClient → ServerCache/ClientCache 
→ Event Dispatch → Storage/Handlers
```

## 实现步骤

### Step 1: 创建 TCPRoute 资源类型定义

**文件**: `src/types/resources/tcp_route.rs` (新建)

```rust
//! TCPRoute resource definition
//!
//! TCPRoute defines TCP rules for mapping requests to backends

use std::fmt;
use std::sync::Arc;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::core::lb::BackendSelector;
use crate::core::filters::PluginRuntime;
use super::http_route_preparse::BackendExtensionInfo;

/// API group for TCPRoute
pub const TCP_ROUTE_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for TCPRoute
pub const TCP_ROUTE_KIND: &str = "TCPRoute";

/// TCPRoute defines TCP rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha2",
    kind = "TCPRoute",
    plural = "tcproutes",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct TCPRouteSpec {
    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Rules defines the TCP routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<TCPRouteRule>>,
}

/// ParentReference identifies a parent resource (usually Gateway)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParentReference {
    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is the kind of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Namespace is the namespace of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Name is the name of the referent
    pub name: String,

    /// SectionName is the name of a section within the target resource
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_name: Option<String>,

    /// Port is the network port this Route targets
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
}

/// TCPRouteRule defines TCP routing rules
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TCPRouteRule {
    /// BackendRefs defines the backends where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<TCPBackendRef>>,

    /// Backend finder for load balancing (not serialized/deserialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_finder: BackendSelector<TCPBackendRef>,

    /// Filter runtime (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,
}

impl Clone for TCPRouteRule {
    fn clone(&self) -> Self {
        Self {
            backend_refs: self.backend_refs.clone(),
            backend_finder: BackendSelector::new(),
            plugin_runtime: self.plugin_runtime.clone(),
        }
    }
}

impl fmt::Debug for TCPRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TCPRouteRule")
            .field("backend_refs", &self.backend_refs)
            .field("backend_finder", &"<skipped>")
            .field("plugin_runtime", &self.plugin_runtime)
            .finish()
    }
}

/// TCPBackendRef defines a backend for TCP traffic
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TCPBackendRef {
    /// Name is the name of the backend Service
    pub name: String,

    /// Namespace is the namespace of the backend Service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Port specifies the destination port number
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,

    /// Weight specifies the proportion of requests forwarded to the backend
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,

    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is the kind of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Parsed extension info (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub extension_info: BackendExtensionInfo,

    /// Filter runtime (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,
}
```

**关键点**:
- 注意 API 版本是 `v1alpha2`，不是 `v1`
- TCPRoute 没有 hostnames 字段（TCP 层面无法匹配 host）
- TCPRouteRule 比 HTTPRouteRule 简单，没有 matches、filters、timeouts 等字段
- 保留 backend_finder 和 plugin_runtime 以支持负载均衡和插件

### Step 2: 实现 ResourceMeta Trait

**文件**: `src/types/resource_meta_traits/tcp_route.rs` (新建)

```rust
//! ResourceMeta implementation for TCPRoute

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::TCPRoute;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for TCPRoute {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::TCPRoute
    }
    
    fn kind_name() -> &'static str {
        "TCPRoute"
    }
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
    
    fn pre_parse(&mut self) {
        // TCPRoute 目前不需要特殊的预解析
        // 未来如果需要可以在这里添加
    }
}
```

**文件**: `src/types/resource_meta_traits/mod.rs` (修改)

添加模块声明：

```rust
mod tcp_route;
```

### Step 3: 更新 ResourceKind 枚举

**文件**: `src/types/resource_kind.rs` (修改)

添加 TCPRoute 变体：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ::prost::Enumeration)]
#[repr(i32)]
pub enum ResourceKind {
    Unspecified = 0,
    GatewayClass = 1,
    EdgionGatewayConfig = 2,
    Gateway = 3,
    HTTPRoute = 4,
    Service = 5,
    EndpointSlice = 6,
    EdgionTls = 7,
    Secret = 8,
    EdgionPlugins = 9,
    GRPCRoute = 10,
    TCPRoute = 11,  // 新增
}

impl ResourceKind {
    pub fn from_kind_name(kind_str: &str) -> Option<Self> {
        match kind_str {
            "Unspecified" => Some(ResourceKind::Unspecified),
            "GatewayClass" => Some(ResourceKind::GatewayClass),
            "EdgionGatewayConfig" => Some(ResourceKind::EdgionGatewayConfig),
            "Gateway" => Some(ResourceKind::Gateway),
            "HTTPRoute" => Some(ResourceKind::HTTPRoute),
            "Service" => Some(ResourceKind::Service),
            "EndpointSlice" => Some(ResourceKind::EndpointSlice),
            "EdgionTls" => Some(ResourceKind::EdgionTls),
            "Secret" => Some(ResourceKind::Secret),
            "EdgionPlugins" => Some(ResourceKind::EdgionPlugins),
            "GRPCRoute" => Some(ResourceKind::GRPCRoute),
            "TCPRoute" => Some(ResourceKind::TCPRoute),  // 新增
            _ => None,
        }
    }
}
```

**文件**: `src/types/resources/mod.rs` (修改)

添加模块和导出：

```rust
pub mod tcp_route;
pub use self::tcp_route::*;
```

### Step 4: 更新 Protocol Buffer 定义

**文件**: `src/core/conf_sync/proto/config_sync.proto` (修改)

添加到 ResourceKind 枚举：

```protobuf
enum ResourceKind {
    RESOURCE_KIND_UNSPECIFIED = 0;
    RESOURCE_KIND_GATEWAY_CLASS = 1;
    RESOURCE_KIND_GATEWAY_CLASS_SPEC = 2;
    RESOURCE_KIND_GATEWAY = 3;
    RESOURCE_KIND_HTTP_ROUTE = 4;
    RESOURCE_KIND_SERVICE = 5;
    RESOURCE_KIND_ENDPOINT_SLICE = 6;
    RESOURCE_KIND_EDGION_TLS = 7;
    RESOURCE_KIND_SECRET = 8;
    RESOURCE_KIND_GRPC_ROUTE = 10;
    RESOURCE_KIND_TCP_ROUTE = 11;  // 新增
}
```

修改后重新编译以生成 proto 绑定：

```bash
cargo build
```

### Step 5: 更新服务端配置同步

**文件**: `src/core/conf_sync/conf_server/config_server.rs` (修改)

1. 添加到 `ResourceItem` 枚举：

```rust
pub enum ResourceItem {
    GatewayClass(GatewayClass),
    EdgionGatewayConfig(EdgionGatewayConfig),
    Gateway(Gateway),
    HTTPRoute(HTTPRoute),
    GRPCRoute(GRPCRoute),
    TCPRoute(TCPRoute),  // 新增
    Service(Service),
    EndpointSlice(EndpointSlice),
    EdgionTls(EdgionTls),
    EdgionPlugins(EdgionPlugins),
    Secret(Secret),
}
```

2. 添加到 `ConfigServer` 结构体：

```rust
pub struct ConfigServer {
    pub base_conf: RwLock<GatewayBaseConf>,
    pub routes: ServerCache<HTTPRoute>,
    pub grpc_routes: ServerCache<GRPCRoute>,
    pub tcp_routes: ServerCache<TCPRoute>,  // 新增
    pub services: ServerCache<Service>,
    pub endpoint_slices: ServerCache<EndpointSlice>,
    pub edgion_tls: ServerCache<EdgionTls>,
    pub edgion_plugins: ServerCache<EdgionPlugins>,
    pub secrets: ServerCache<Secret>,
}
```

3. 更新 `new()` 方法：

```rust
impl ConfigServer {
    pub fn new(base_conf: GatewayBaseConf) -> Self {
        Self {
            base_conf: RwLock::new(base_conf),
            routes: ServerCache::new(200),
            grpc_routes: ServerCache::new(200),
            tcp_routes: ServerCache::new(200),  // 新增
            services: ServerCache::new(200),
            endpoint_slices: ServerCache::new(200),
            edgion_tls: ServerCache::new(200),
            edgion_plugins: ServerCache::new(200),
            secrets: ServerCache::new(200),
        }
    }
}
```

4. 更新 `list()` 方法，添加 TCPRoute 分支：

```rust
ResourceKind::TCPRoute => {
    let list_data = self.tcp_routes.list();
    let json = serde_json::to_string(&list_data.data)
        .map_err(|e| format!("Failed to serialize TCPRoute data: {}", e))?;
    (json, list_data.resource_version)
}
```

**文件**: `src/core/conf_sync/conf_server/event_dispatch.rs` (修改)

1. 在 `apply_resource_change_with_resource_type()` 中添加 TCPRoute 处理：

```rust
ResourceKind::TCPRoute => {
    if let Ok(resource) = serde_yaml::from_str::<TCPRoute>(&data) {
        // 检查 TCPRoute 引用的 gateway 是否存在于 base_conf 中
        let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
            if let Some(first_ref) = parent_refs.first() {
                let gateway_namespace = first_ref
                    .namespace
                    .as_ref()
                    .or_else(|| resource.metadata.namespace.as_ref());
                let gateway_name = Some(&first_ref.name);

                let base_conf_guard = self.base_conf.read().unwrap();
                base_conf_guard.has_gateway(gateway_namespace, gateway_name)
            } else {
                false
            }
        } else {
            false
        };

        if !gateway_exists {
            tracing::warn!(
                component = "config_server",
                change = ?change,
                kind = "TCPRoute",
                route_name = ?resource.metadata.name,
                route_namespace = ?resource.metadata.namespace,
                "TCPRoute references a Gateway that does not exist in base_conf, skipping"
            );
            return;
        }

        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "TCPRoute",
            "Applying TCPRoute resource change"
        );
        Self::execute_change_on_cache::<TCPRoute>(change, &self.tcp_routes, resource);
    }
}
```

2. 在 `ConfigServerEventDispatcher` 实现中更新：

```rust
impl ConfigServerEventDispatcher for ConfigServer {
    fn enable_version_fix_mode(&self) {
        self.routes.enable_version_fix_mode();
        self.grpc_routes.enable_version_fix_mode();
        self.tcp_routes.enable_version_fix_mode();  // 新增
        self.services.enable_version_fix_mode();
        self.endpoint_slices.enable_version_fix_mode();
        self.edgion_tls.enable_version_fix_mode();
        self.edgion_plugins.enable_version_fix_mode();
        self.secrets.enable_version_fix_mode();
    }

    fn set_ready(&self) {
        self.routes.set_ready();
        self.grpc_routes.set_ready();
        self.tcp_routes.set_ready();  // 新增
        self.services.set_ready();
        self.endpoint_slices.set_ready();
        self.edgion_tls.set_ready();
        self.edgion_plugins.set_ready();
        self.secrets.set_ready();
    }
}
```

### Step 6: 更新客户端配置同步

**文件**: `src/core/conf_sync/conf_client/config_client.rs` (修改)

1. 添加到 `ConfigClient` 结构体：

```rust
pub struct ConfigClient {
    gateway_class_key: String,
    pub base_conf: RwLock<Option<GatewayBaseConf>>,
    routes: ClientCache<HTTPRoute>,
    grpc_routes: ClientCache<GRPCRoute>,
    tcp_routes: ClientCache<TCPRoute>,  // 新增
    services: ClientCache<Service>,
    endpoint_slices: ClientCache<EndpointSlice>,
    edgion_tls: ClientCache<EdgionTls>,
    edgion_plugins: ClientCache<EdgionPlugins>,
    secrets: ClientCache<Secret>,
}
```

2. 更新 `new()` 方法：

```rust
impl ConfigClient {
    pub fn new(gateway_class_key: String, client_id: String, client_name: String) -> Self {
        // ... 现有的 cache 初始化 ...
        
        let tcp_routes_cache = ClientCache::new(
            gateway_class_key.clone(), 
            client_id.clone(), 
            client_name.clone()
        );
        // 如果需要，可以注册 handler
        // let tcp_route_handler = create_tcp_route_handler();
        // tcp_routes_cache.set_conf_processor(tcp_route_handler);
        
        Self {
            gateway_class_key: gateway_class_key.clone(),
            base_conf: RwLock::new(None),
            routes: routes_cache,
            grpc_routes: grpc_routes_cache,
            tcp_routes: tcp_routes_cache,  // 新增
            services: services_cache,
            endpoint_slices: endpoint_slices_cache,
            edgion_tls: ClientCache::new(gateway_class_key.clone(), client_id.clone(), client_name.clone()),
            edgion_plugins: plugins_cache,
            secrets: ClientCache::new(gateway_class_key, client_id, client_name),
        }
    }
}
```

3. 添加访问方法：

```rust
/// Get tcp_routes cache for direct access
pub fn tcp_routes(&self) -> &ClientCache<TCPRoute> {
    &self.tcp_routes
}

/// List TCP routes
pub fn list_tcp_routes(&self) -> ListData<TCPRoute> {
    self.tcp_routes.list_owned()
}
```

4. 更新 `is_ready()` 方法：

```rust
pub fn is_ready(&self) -> Result<(), String> {
    let mut not_ready = Vec::new();
    
    if !self.routes.is_ready() {
        not_ready.push("routes");
    }
    if !self.grpc_routes.is_ready() {
        not_ready.push("grpc_routes");
    }
    if !self.tcp_routes.is_ready() {  // 新增
        not_ready.push("tcp_routes");
    }
    if !self.services.is_ready() {
        not_ready.push("services");
    }
    // ... 其他检查 ...
    
    if not_ready.is_empty() {
        Ok(())
    } else {
        Err(format!("wait [{}] ready", not_ready.join(", ")))
    }
}
```

5. 更新 `list()` 方法：

```rust
ResourceKind::TCPRoute => {
    let list_data = self.tcp_routes.list();
    let json = serde_json::to_string(&list_data.data)
        .map_err(|e| format!("Failed to serialize TCPRoute data: {}", e))?;
    (json, list_data.resource_version)
}
```

6. 更新 `print_config()` 方法：

```rust
// TCP Routes
let list_data = self.list_tcp_routes();
println!(
    "TCPRoutes (count: {}, version: {}):",
    list_data.data.len(),
    list_data.resource_version
);
for (idx, route) in list_data.data.iter().enumerate() {
    println!("  [{}] {}", idx, format_resource_info(route));
}
```

7. 在 `ConfigClientEventDispatcher` 实现中添加：

```rust
impl ConfigClientEventDispatcher for ConfigClient {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        _resource_version: Option<u64>,
    ) {
        // ... 现有的匹配分支 ...
        
        ResourceKind::TCPRoute => match serde_yaml::from_str::<TCPRoute>(&data) {
            Ok(resource) => {
                Self::apply_change_to_cache(&self.tcp_routes, change, resource);
            }
            Err(e) => log_error("TCPRoute", &e),
        },
        
        // ... 其他分支 ...
    }
}
```

### Step 7: 检查其他需要更新的文件

**文件**: `src/types/prelude_resources.rs` (如果存在，添加导出)

```rust
pub use crate::types::resources::TCPRoute;
```

### Step 8: 创建示例 YAML

**文件**: `config/examples/test_tcp-route_TCPRoute.yaml` (新建)

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: example-tcp-route
  namespace: default
spec:
  parentRefs:
    - name: gateway1
      namespace: default
      sectionName: tcp-listener
  rules:
    - backendRefs:
        - name: tcp-service
          port: 3306
          weight: 1
```

## TCPRoute vs HTTPRoute/GRPCRoute 的关键区别

| 特性 | HTTPRoute | GRPCRoute | TCPRoute |
|------|-----------|-----------|----------|
| API 版本 | v1 | v1 | v1alpha2 |
| Hostnames | ✅ 支持 | ✅ 支持 | ❌ 不支持 |
| 路径匹配 | ✅ 支持 | ❌ 不支持 | ❌ 不支持 |
| 方法匹配 | ✅ HTTP方法 | ✅ gRPC方法 | ❌ 不支持 |
| Header 匹配 | ✅ 支持 | ✅ 支持 | ❌ 不支持 |
| Filters | ✅ 多种 | ✅ 部分 | ❌ 不支持 |
| Timeouts | ✅ 支持 | ✅ 支持 | ❌ 不支持 |
| Retry | ✅ 支持 | ✅ 支持 | ❌ 不支持 |
| Session Persistence | ✅ 支持 | ✅ 支持 | ❌ 不支持 |
| Backend Refs | ✅ 支持 | ✅ 支持 | ✅ 支持 |
| 负载均衡 | ✅ 支持 | ✅ 支持 | ✅ 支持 |

## 测试清单

实现完成后，测试以下内容：

- [ ] 创建示例 TCPRoute YAML 并应用
- [ ] 验证 operator 能正确加载 TCPRoute
- [ ] 测试 List 操作返回 TCPRoute 列表
- [ ] 测试 Watch 操作响应 Add/Update/Delete 事件
- [ ] 验证 parent gateway 引用验证逻辑
- [ ] 测试负载均衡功能
- [ ] 验证缓存同步正常工作
- [ ] 检查日志输出正确

## 实现文件清单

**新增文件** (2):
- `src/types/resources/tcp_route.rs`
- `src/types/resource_meta_traits/tcp_route.rs`
- `config/examples/test_tcp-route_TCPRoute.yaml`

**修改文件** (~8):
- `src/types/resource_kind.rs`
- `src/types/resources/mod.rs`
- `src/types/resource_meta_traits/mod.rs`
- `src/core/conf_sync/proto/config_sync.proto`
- `src/core/conf_sync/conf_server/config_server.rs`
- `src/core/conf_sync/conf_server/event_dispatch.rs`
- `src/core/conf_sync/conf_client/config_client.rs`
- 可能需要: `src/types/prelude_resources.rs`

## 注意事项

1. **API 版本**: TCPRoute 使用 `v1alpha2`，不是 `v1`
2. **简化结构**: TCPRoute 的规则比 HTTP/gRPC 路由简单得多
3. **无匹配条件**: TCPRoute 没有 matches 字段，路由主要依赖 Gateway listener 的配置
4. **插件支持**: 虽然保留了 plugin_runtime，但要注意 TCP 层面能做的操作有限
5. **TLS**: TCPRoute 可以配合 TLS listener 使用，但 TLS 终止在 Gateway 层面

## 参考资料

- [Kubernetes Gateway API TCPRoute Specification](https://gateway-api.sigs.k8s.io/api-types/tcproute/)
- [HTTPRoute 实现](../src/types/resources/http_route.rs)
- [GRPCRoute 实现](../src/types/resources/grpc_route.rs)
- [Resource Architecture Overview](./resource-architecture-overview.md)

