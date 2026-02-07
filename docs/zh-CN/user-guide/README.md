# 用户指南 (User Guide)

本章节面向应用开发者，介绍如何使用 Gateway API 配置路由规则。

## 目录

### HTTPRoute
- [HTTPRoute 总览](./http-route/overview.md)
- **匹配规则 (Matches)**
  - [路径匹配](./http-route/matches/path.md)
  - [请求头匹配](./http-route/matches/headers.md)
  - [查询参数匹配](./http-route/matches/query-params.md)
  - [HTTP 方法匹配](./http-route/matches/method.md)
- **过滤器 (Filters)**
  - [过滤器总览](./http-route/filters/overview.md)
  - [插件组合与引用 🔌](./http-route/filters/plugin-composition.md)
  - Gateway API 标准过滤器
    - [RequestHeaderModifier](./http-route/filters/gateway-api/request-header-modifier.md)
    - [ResponseHeaderModifier](./http-route/filters/gateway-api/response-header-modifier.md)
    - [RequestRedirect](./http-route/filters/gateway-api/request-redirect.md)
    - [URLRewrite](./http-route/filters/gateway-api/url-rewrite.md)
  - Edgion 扩展插件 🔌
    - [BasicAuth](./http-route/filters/edgion-plugins/basic-auth.md)
    - [CORS](./http-route/filters/edgion-plugins/cors.md)
    - [CSRF](./http-route/filters/edgion-plugins/csrf.md)
    - [CtxSet](./edgion-plugins/ctx-setter.md)
    - [IP 限制](./http-route/filters/edgion-plugins/ip-restriction.md)
    - [ProxyRewrite](./http-route/filters/edgion-plugins/proxy-rewrite.md)
    - [限流](./http-route/filters/edgion-plugins/rate-limit.md)
- **后端配置 (Backends)**
  - [Service 引用](./http-route/backends/service-ref.md)
  - [权重配置](./http-route/backends/weight.md)
  - [后端 TLS](./http-route/backends/backend-tls.md)
- **弹性配置 (Resilience)**
  - [超时配置](./http-route/resilience/timeouts.md)
  - [重试策略](./http-route/resilience/retry.md)
  - [会话保持](./http-route/resilience/session-persistence.md)
- [负载均衡算法 🔌](./http-route/lb-algorithms.md)

### GRPCRoute
- [GRPCRoute 总览](./grpc-route/overview.md)
- [匹配规则](./grpc-route/matches/)
- [过滤器](./grpc-route/filters/)
- [后端配置](./grpc-route/backends/)

### TCPRoute
- [TCPRoute 总览](./tcp-route/overview.md)
- [后端配置](./tcp-route/backends/)
- [Stream Plugins 🔌](./tcp-route/stream-plugins.md)

### UDPRoute
- [UDPRoute 总览](./udp-route/overview.md)
- [后端配置](./udp-route/backends/)
- [Stream Plugins](./tcp-route/stream-plugins.md) *(同 TCPRoute)*

### 高级特性
- [灰度发布](./advanced/canary-release.md)
- [蓝绿部署](./advanced/blue-green.md)