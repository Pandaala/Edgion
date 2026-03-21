---
name: resource-grpc-route
description: GRPCRoute 资源：gRPC Service/Method 匹配、gRPC-Web、与 HTTPRoute 的关系。
---

# GRPCRoute 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 待填充内容

### 功能点

<!-- TODO:
- 定义 gRPC 路由规则（hostnames, rules, matches, filters, backendRefs）
- 支持 Service/Method 精确和前缀匹配
- 支持 gRPC-Web 协议
- 支持 Header 匹配
-->

### Controller 侧处理

<!-- TODO: GrpcRouteHandler，与 HttpRouteHandler 共享 route_utils -->

### Gateway 侧处理

<!-- TODO: 复用 HTTP per-port 隔离和 domain 匹配 -->

### 跨资源关联

<!-- TODO: 与 HTTPRoute 类似：→ Gateway, → Service, → EdgionPlugins, → ReferenceGrant -->
