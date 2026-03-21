---
name: resource-service-endpoints
description: Service + EndpointSlice + Endpoint 资源：后端发现、级联 requeue、两种端点模式。
---

# Service + EndpointSlice + Endpoint 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

这三种 Kubernetes 核心资源共同构成后端服务发现机制。Service 定义逻辑服务，EndpointSlice（优先）和 Endpoint（遗留）提供实际的后端 Pod IP 和端口。

## 源码位置

- Controller Handlers:
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/service.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/endpoint_slice.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/endpoints.rs`
- Gateway ConfHandlers:
  - `src/core/gateway/backends/discovery/services/conf_handler_impl.rs`
  - `src/core/gateway/backends/discovery/endpoint_slice/conf_handler_impl.rs`
  - `src/core/gateway/backends/discovery/endpoint/conf_handler_impl.rs`

## Service

### Controller 侧处理

#### parse

无特殊处理逻辑，直接透传。

#### on_change / on_delete

通过 `ServiceRefManager` 触发级联 requeue：查找所有以此 Service 作为 backendRef 的路由资源（HTTPRoute、GRPCRoute、TCPRoute、TLSRoute、UDPRoute），逐一 requeue。这确保：
- Service 创建时：之前 ResolvedRefs=False 的路由重新校验变为 True
- Service 删除时：路由的 ResolvedRefs 从 True 变为 False
- Service 更新时：路由重新解析后端配置

### Gateway 侧处理

Service 同步到 Gateway 后用于后端发现的元数据（端口映射、协议等）。

## EndpointSlice（优先模式）

### Controller 侧处理

#### parse

无特殊处理逻辑，直接透传。EndpointSlice 数据在 Gateway 侧消费。

### Gateway 侧处理

EndpointSlice 是 Kubernetes 推荐的端点发现机制，提供 Service 的后端 Pod 地址列表。Gateway 侧从 EndpointSlice 构建负载均衡目标池，用于请求转发时的后端选择和健康检查目标列表。

## Endpoint（遗留模式）

### Controller 侧处理

#### parse

无特殊处理逻辑，直接透传。

### Gateway 侧处理

Endpoint 是 Kubernetes 遗留的端点发现机制。当 EndpointSlice 不可用时回退使用。功能与 EndpointSlice 等价，但性能和扩展性较差（不支持分片）。

## ServiceRefManager 机制

路由资源（HTTPRoute 等）在 parse 阶段通过 `register_service_backend_refs()` 将 backendRef 中的 Service 引用注册到 ServiceRefManager。Service 变更时，ServiceHandler 调用 `get_service_ref_manager().get_refs()` 获取依赖列表并 requeue。

注册生命周期：
- 路由 parse 时注册
- 路由 delete 时通过 `clear_service_backend_refs()` 清除
- 路由 update 时先清除旧注册再注册新引用

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| Service ← HTTPRoute | HTTPRoute | backendRef + ServiceRefManager | Service 变更 requeue 路由 |
| Service ← GRPCRoute | GRPCRoute | backendRef + ServiceRefManager | 同上 |
| Service ← TCPRoute | TCPRoute | backendRef + ServiceRefManager | 同上 |
| Service ← TLSRoute | TLSRoute | backendRef + ServiceRefManager | 同上 |
| Service ← UDPRoute | UDPRoute | backendRef + ServiceRefManager | 同上 |
| Service ← BackendTLSPolicy | BackendTLSPolicy | targetRef | 策略绑定目标 |
| EndpointSlice → Service | Service | kubernetes.io/service-name label | 关联到 Service |
| Endpoint → Service | Service | 同名 | 与 Service 同名关联 |
