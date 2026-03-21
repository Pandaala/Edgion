---
name: resource-gateway
description: Gateway 资源架构：Listener 定义、端口管理、TLS 证书绑定、Route 挂载、Status 管理。
---

# Gateway 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 待填充内容

### 功能点

<!-- TODO:
- 定义 Listener（端口、协议、hostname、TLS 配置）
- 管理端口分配和冲突检测（ListenerPortManager）
- 绑定 TLS 证书（certificateRefs → Secret/EdgionTls）
- 接受 Route 挂载（AttachedRouteTracker）
-->

### Controller 侧处理

<!-- TODO:
- GatewayHandler: 校验 GatewayClass、Listener 端口冲突、TLS 引用
- 维护 gateway_route_index（Gateway ↔ Route 关系）
- Listener 状态管理（Accepted/ResolvedRefs/Programmed）
-->

### Gateway 侧处理

<!-- TODO:
- 更新 GatewayBase 的 Listener 配置
- 重建 Pingora listener
- 更新 per-port 路由表
- 更新 TLS Store 绑定
-->

### 跨资源关联

<!-- TODO:
- → GatewayClass: 必须引用有效的 GatewayClass
- → Secret: TLS certificateRefs
- → EdgionTls: 扩展 TLS 配置
- → HTTPRoute/GRPCRoute/TCPRoute/TLSRoute/UDPRoute: Route 挂载
- → EdgionGatewayConfig: 全局配置覆盖
- ← ListenerPortManager: 端口冲突检测（Gateway ↔ Gateway）
-->

### Status 字段

<!-- TODO: Listeners[].conditions, Addresses, AttachedRoutes 等 -->
