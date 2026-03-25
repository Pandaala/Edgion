---
name: resource-tls-route
description: TLSRoute 资源：TLS SNI 路由、passthrough/terminate 模式、per-port 路由表。
---

# TLSRoute 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

TLSRoute 定义 TLS 层的路由规则，支持 TLS passthrough 和 terminate 模式。通过 SNI（Server Name Indication）进行路由匹配，每个端口维护独立的路由表。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs`
- Gateway ConfHandler: `src/core/gateway/routes/tls/conf_handler_impl.rs`
- 路由管理器: `src/core/gateway/routes/tls/routes_mgr.rs`（GlobalTlsRouteManagers）
- 类型定义: `src/types/resources/tls_route.rs`

## Controller 侧处理

### validate

1. 调用 `validate_tls_route_if_enabled()` 执行 ReferenceGrant 校验
2. 调用 `validate_backend_refs()` 校验所有 backendRef

### parse

1. 记录跨命名空间引用到 CrossNsRefManager
2. 注册 Service 引用到 ServiceRefManager
3. 标记被拒绝的跨命名空间 backendRef
4. 清除并重算 `resolved_ports`：从 parentRef 解析 Listener 端口，遵循 Gateway API 规范：
   - parentRef.port 已指定 → 直接使用
   - parentRef.sectionName 已指定 → 查找匹配 Listener 获取端口
   - 都未指定 → 挂载到父 Gateway 的所有 Listener（受 allowedRoutes 命名空间策略约束）
5. 若 resolved_ports 为 None，记录 debug 日志

### on_change

注册到 GatewayRouteIndex，更新 AttachedRouteTracker，必要时 requeue 父 Gateway。

### on_delete

清除 CrossNsRefManager、ServiceRefManager、GatewayRouteIndex、AttachedRouteTracker 注册。

### update_status

为每个 parentRef 生成 RouteParentStatus。TLSRoute 会传递 route_hostnames 用于 hostname 交集检查（SNI 匹配需要）。

## Gateway 侧处理

TLSRoute 的 ConfHandler 将路由编译到 `GlobalTlsRouteManagers`：

1. **路由初始化**：调用 `initialize_route()` 验证路由数据
2. **per-port 管理**：根据 resolved_ports 注册到对应端口的路由管理器
3. **SNI 匹配**：根据 spec.hostnames 进行 TLS SNI 匹配，支持通配符域名
4. **增量更新**：partial_update 时收集受影响端口（旧端口 + 新端口），仅重建受影响端口的路由表

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| TLSRoute → Gateway | Gateway | parentRef | 路由挂载到 Gateway 的 TLS 协议 Listener |
| TLSRoute → Service | Service | backendRef | TLS 连接转发的后端目标 |
| TLSRoute ← ReferenceGrant | ReferenceGrant | 跨命名空间 backendRef | 控制跨命名空间后端引用的授权 |
