---
name: gateway-runtime-store
description: Gateway 运行时存储：Gateway 资源存储、Port-Gateway 信息、Listener 配置、Route 配置存储。
---

# 运行时存储

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### Gateway 存储

<!-- TODO:
runtime/store/gateway/
- Gateway 资源的运行时表示
- 按 namespace/name 索引
-->

### Port-Gateway 信息

<!-- TODO:
runtime/store/port_gateway_info/
- Per-port Gateway 信息，用于路由匹配时的 Gateway/Listener 约束检查
-->

### Listener 配置存储

<!-- TODO:
runtime/store/config/
- Gateway Listener 配置的运行时存储
-->

### Gateway 匹配

<!-- TODO:
runtime/matching/route/
- Gateway listener 匹配逻辑
-->

### TLS 匹配

<!-- TODO:
runtime/matching/tls/
- 基于 SNI 的 Gateway TLS 证书匹配
-->

### GatewayClass 存储

<!-- TODO: config/gateway_class/ — GatewayClass 存储和处理 -->

### EdgionGatewayConfig 存储

<!-- TODO: config/edgion_gateway/ — EdgionGatewayConfig 存储和处理 -->
