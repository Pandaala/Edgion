# Mesh Conformance Tests (21 tests)

> **注意**: Mesh profile 用于服务网格场景，不使用 Gateway 资源，而是通过 parentRef 直接引用 Service。

## Core Tests (7 tests)

### 1. MeshBasic
- **Features**: Mesh
- **Manifest**: `mesh/mesh-basic.yaml`
- **描述**: 基本 Mesh 路由
- **测试逻辑**:
  1. 创建 HTTPRoute，parentRef 为 Service (非 Gateway)
  2. 验证 mesh 内流量按路由规则转发
  3. 验证基本的 service-to-service 路由

### 2. MeshHTTPRouteSimpleSameNamespace
- **Features**: Mesh, HTTPRoute
- **Manifest**: `mesh/mesh-httproute-simple-same-namespace.yaml`
- **描述**: Mesh 内同命名空间 HTTPRoute
- **测试逻辑**:
  1. HTTPRoute parentRef 指向同命名空间 Service
  2. 从 mesh 内发送请求到该 Service
  3. 验证请求按 HTTPRoute 规则路由

### 3. MeshHTTPRouteMatching
- **Features**: Mesh, HTTPRoute
- **Manifest**: `mesh/mesh-httproute-matching.yaml`
- **描述**: Mesh 内 HTTPRoute 匹配规则
- **测试逻辑**:
  1. 创建 HTTPRoute，包含路径和 header 匹配规则
  2. 从 mesh 客户端发送不同路径/header 的请求
  3. 验证每个请求路由到正确的后端

### 4. MeshHTTPRouteRedirectHostAndStatus
- **Features**: Mesh, HTTPRoute
- **Manifest**: `mesh/mesh-httproute-redirect-host-and-status.yaml`
- **描述**: Mesh 内 Host 重定向
- **测试逻辑**:
  1. HTTPRoute 配置 RequestRedirect filter
  2. 从 mesh 内发送请求
  3. 验证收到重定向响应 (Location header + 状态码)

### 5. MeshHTTPRouteWeight
- **Features**: Mesh, HTTPRoute
- **Manifest**: `mesh/mesh-httproute-weight.yaml`
- **描述**: Mesh 内加权路由
- **测试逻辑**:
  1. HTTPRoute 有多个 backendRef 带不同 weight
  2. 发送大量请求
  3. 验证按 weight 比例分配

### 6. MeshTrafficSplit
- **Features**: Mesh, HTTPRoute
- **Manifest**: `mesh/mesh-split.yaml`
- **描述**: Mesh 流量拆分
- **测试逻辑**:
  1. HTTPRoute 实现金丝雀发布场景
  2. 流量按比例分配到不同版本后端
  3. 验证分配比例正确

### 7. MeshFrontend
- **Features**: Mesh, HTTPRoute, HTTPRouteResponseHeaderModification
- **Manifest**: `mesh/mesh-frontend.yaml`
- **描述**: Mesh 前端路由
- **测试逻辑**:
  1. 前端 Service 通过 HTTPRoute 路由到后端
  2. 验证 response header modification 在 mesh 场景中正常工作
  3. 验证 mesh 流量链路

---

## Extended Tests (14 tests)

### 8. MeshPorts
- **Features**: Mesh, HTTPRoute, HTTPRouteParentRefPort, HTTPRouteResponseHeaderModification
- **Manifest**: `mesh/mesh-ports.yaml`
- **描述**: Mesh 端口级路由
- **测试逻辑**:
  1. HTTPRoute parentRef 指定特定端口
  2. Service 有多个端口
  3. 只有指定端口的流量按路由规则处理
  4. 其他端口流量不受影响

### 9. MeshHTTPRouteBackendRequestHeaderModifier
- **Features**: Mesh, HTTPRoute, MeshHTTPRouteBackendRequestHeaderModification
- **Manifest**: `mesh/mesh-httproute-backend-request-header-modifier.yaml`
- **描述**: Mesh 内后端请求 header 修改
- **测试逻辑**:
  1. HTTPRoute 配置 backend-level RequestHeaderModifier
  2. 验证后端收到的 header 被正确修改 (set/add/remove)

### 10. MeshHTTPRouteRewritePath
- **Features**: Mesh, HTTPRoute, MeshHTTPRouteRewritePath
- **Manifest**: `mesh/mesh-httproute-rewrite-path.yaml`
- **描述**: Mesh 内路径重写
- **测试逻辑**:
  1. HTTPRoute 配置 URLRewrite filter (路径)
  2. 验证后端收到的请求路径被重写

### 11. MeshHTTPRouteSchemeRedirect
- **Features**: Mesh, HTTPRoute, MeshHTTPRouteSchemeRedirect
- **Manifest**: `mesh/mesh-httproute-scheme-redirect.yaml`
- **描述**: Mesh 内 scheme 重定向
- **测试逻辑**:
  1. HTTPRoute 配置 scheme redirect (http→https)
  2. 验证 Location header 中的 scheme 正确

### 12. MeshHTTPRouteRedirectPort
- **Features**: Mesh, HTTPRoute, MeshHTTPRouteRedirectPort
- **Manifest**: `mesh/mesh-httproute-redirect-port.yaml`
- **描述**: Mesh 内端口重定向
- **测试逻辑**:
  1. HTTPRoute 配置端口重定向
  2. 验证 Location header 中的端口正确

### 13. MeshHTTPRouteRedirectPath
- **Features**: Mesh, HTTPRoute, MeshHTTPRouteRedirectPath
- **Manifest**: `mesh/mesh-httproute-redirect-path.yaml`
- **描述**: Mesh 内路径重定向
- **测试逻辑**:
  1. HTTPRoute 配置路径重定向
  2. 验证 Location header 中的路径正确

### 14. MeshHTTPRouteQueryParamMatching
- **Features**: Mesh, HTTPRoute, MeshHTTPRouteQueryParamMatching
- **Manifest**: `mesh/mesh-httproute-query-param-matching.yaml`
- **描述**: Mesh 内查询参数匹配
- **测试逻辑**:
  1. HTTPRoute 基于 query parameter 匹配
  2. 不同参数值路由到不同后端

### 15. MeshHTTPRouteNamedRule
- **Features**: Mesh, HTTPRoute, MeshHTTPRouteNamedRouteRule
- **Manifest**: `mesh/mesh-httproute-named-rule.yaml`
- **描述**: Mesh 内命名路由规则
- **测试逻辑**:
  1. HTTPRoute rules 带 name 字段
  2. 验证路由正常工作

### 16. MeshConsumerRoute
- **Features**: Mesh, MeshConsumerRoute, HTTPRoute, HTTPRouteResponseHeaderModification
- **Manifest**: `mesh/mesh-consumer-route.yaml`
- **描述**: Mesh 消费者路由 (调用方视角的路由规则)
- **测试逻辑**:
  1. HTTPRoute 作为消费者路由，影响特定调用方的流量
  2. 不同消费者看到不同的路由行为
  3. 验证消费者身份影响路由决策

### 17. MeshFrontendHostname
- **Features**: Mesh, MeshClusterIPMatching, HTTPRoute, HTTPRouteResponseHeaderModification
- **Manifest**: `mesh/mesh-frontend-hostname.yaml`
- **描述**: Mesh 前端 hostname/ClusterIP 匹配
- **测试逻辑**:
  1. HTTPRoute parentRef 引用 Service
  2. 请求通过 ClusterIP 到达
  3. 验证 ClusterIP 匹配正确触发路由

### 18. MeshGRPCRouteWeight
- **Features**: Mesh, GRPCRoute
- **Manifest**: `mesh/mesh-grpcroute-weight.yaml`
- **描述**: Mesh 内 gRPC 加权路由
- **测试逻辑**:
  1. GRPCRoute parentRef 指向 Service
  2. 多个后端 backendRef 带不同 weight
  3. 验证 gRPC 流量按 weight 分配

---

## Mesh Redirect Status Code Tests (3 tests)

### 19. MeshHTTPRoute303Redirect
- **Features**: Mesh, HTTPRoute, HTTPRoute303RedirectStatusCode
- **Manifest**: `mesh/mesh-httproute-303-redirect.yaml`
- **描述**: Mesh 内 303 重定向
- **测试逻辑**: 验证 mesh 场景下 303 状态码重定向

### 20. MeshHTTPRoute307Redirect
- **Features**: Mesh, HTTPRoute, HTTPRoute307RedirectStatusCode
- **Manifest**: `mesh/mesh-httproute-307-redirect.yaml`
- **描述**: Mesh 内 307 重定向
- **测试逻辑**: 验证 mesh 场景下 307 状态码重定向 (保留方法)

### 21. MeshHTTPRoute308Redirect
- **Features**: Mesh, HTTPRoute, HTTPRoute308RedirectStatusCode
- **Manifest**: `mesh/mesh-httproute-308-redirect.yaml`
- **描述**: Mesh 内 308 重定向
- **测试逻辑**: 验证 mesh 场景下 308 状态码重定向 (永久，保留方法)
