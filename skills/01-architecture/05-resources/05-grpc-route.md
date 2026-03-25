---
name: resource-grpc-route
description: GRPCRoute 资源：gRPC Service/Method 匹配、gRPC-Web、与 HTTPRoute 的关系。
---

# GRPCRoute 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

GRPCRoute 定义 gRPC 层的路由规则。其 Controller 侧处理逻辑与 HTTPRoute 高度一致（共享 route_utils 工具函数），主要区别在于 Gateway 侧的匹配方式：使用 gRPC Service 名和 Method 名进行匹配，而非 HTTP 路径。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs`
- Gateway ConfHandler: `src/core/gateway/routes/grpc/conf_handler_impl.rs`
- 路由管理器: `src/core/gateway/routes/grpc/routes_mgr.rs`
- 匹配单元: `src/core/gateway/routes/grpc/match_unit.rs`
- 类型定义: `src/types/resources/grpc_route.rs`

## Controller 侧处理

### validate

1. 调用 `validate_grpc_route_if_enabled()` 执行 ReferenceGrant 校验
2. 调用 `validate_backend_refs()` 校验所有 backendRef

### parse

与 HTTPRoute 处理模式完全一致：
1. 记录跨命名空间引用到 CrossNsRefManager
2. 注册 Service 引用到 ServiceRefManager
3. 清除并重算 `resolved_ports`（从 parentRef 解析 Listener 端口）
4. 解析 `resolved_hostnames`（路由 hostnames 与 Gateway Listener hostnames 的交集）
5. 标记被拒绝的跨命名空间 backendRef（设置 `ref_denied`）

### on_change

与 HTTPRoute 相同：注册 GatewayRouteIndex，更新 AttachedRouteTracker，必要时 requeue 父 Gateway。

### on_delete

与 HTTPRoute 相同：清除所有引用和索引注册。

### update_status

与 HTTPRoute 相同：为每个 parentRef 生成 RouteParentStatus，设置 Accepted 和 ResolvedRefs Conditions。

## Gateway 侧处理

GRPCRoute 的 ConfHandler 将路由编译到 `GlobalGrpcRouteManagers`：

1. **preparse**：在路由缓存中调用 `route.preparse()` 构建运行时结构
2. **路由单元**：每条规则编译为 `GrpcRouteRuleUnit`，包含 `GrpcMatchInfo`（service、method、headers 匹配）
3. **域名匹配**：使用 resolved_hostnames（优先）或 spec.hostnames 作为匹配域名，无 hostname 时使用 `*` 全匹配
4. **per-port 管理**：根据 resolved_ports 注册到对应端口的路由管理器，变更时仅重建受影响端口的路由表
5. **gRPC-Web 支持**：通过 content-type 检测自动支持 gRPC-Web 协议

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| GRPCRoute → Gateway | Gateway | parentRef | 路由挂载到 Gateway 的 Listener |
| GRPCRoute → Service | Service | backendRef | 路由的后端目标服务 |
| GRPCRoute → EdgionPlugins | EdgionPlugins | ExtensionRef filter | 通过 ExtensionRef 引用插件配置 |
| GRPCRoute ← ReferenceGrant | ReferenceGrant | 跨命名空间 backendRef | 控制跨命名空间后端引用的授权 |
