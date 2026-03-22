# Gateway API v1.5.0 Conformance Tests - 完整测试内容

本目录包含 Gateway API v1.5.0 官方 conformance 测试套件的所有测试用例详细说明。
用于逐一核对 Edgion 网关实现的功能完整性。

## 测试总览

**总计: 128 个测试用例**

| 类别 | 数量 | 文件 |
|------|------|------|
| HTTPRoute | 52 | [01-httproute-tests.md](01-httproute-tests.md) |
| Gateway / GatewayClass | 21 | [02-gateway-tests.md](02-gateway-tests.md) |
| TLSRoute | 13 | [03-tlsroute-tests.md](03-tlsroute-tests.md) |
| GRPCRoute | 5 | [04-grpcroute-tests.md](04-grpcroute-tests.md) |
| BackendTLSPolicy | 6 | [05-backend-tls-policy-tests.md](05-backend-tls-policy-tests.md) |
| ListenerSet | 10 | [06-listenerset-tests.md](06-listenerset-tests.md) |
| Mesh | 21 | [07-mesh-tests.md](07-mesh-tests.md) |
| UDPRoute | 1 | [08-udproute-tests.md](08-udproute-tests.md) |

## Conformance Profiles

| Profile | Core Features | 说明 |
|---------|--------------|------|
| GATEWAY-HTTP | Gateway, ReferenceGrant, HTTPRoute | HTTP 网关路由 |
| GATEWAY-TLS | Gateway, ReferenceGrant, TLSRoute | TLS 网关路由 |
| GATEWAY-GRPC | Gateway, ReferenceGrant, GRPCRoute | gRPC 网关路由 |
| MESH-HTTP | Mesh, HTTPRoute | 服务网格 HTTP |
| MESH-GRPC | Mesh, GRPCRoute | 服务网格 gRPC |

## Extended Features 清单

### Gateway Extended
- `GatewayPort8080` - 非标准端口监听
- `GatewayStaticAddresses` - 静态地址分配
- `GatewayHTTPListenerIsolation` - HTTP 监听器隔离
- `GatewayHTTPSListenerDetectMisdirectedRequests` - HTTPS 错误路由检测
- `GatewayInfrastructurePropagation` - 基础设施传播
- `GatewayAddressEmpty` - 空地址支持
- `ListenerSet` - 监听器集合 (v1.5 新增)
- `GatewayBackendClientCertificate` - 后端客户端证书
- `GatewayFrontendClientCertificateValidation` - 前端客户端证书验证
- `GatewayFrontendClientCertificateValidationInsecureFallback` - 前端证书验证不安全回退

### HTTPRoute Extended
- `HTTPRouteQueryParamMatching` - 查询参数匹配
- `HTTPRouteMethodMatching` - HTTP 方法匹配
- `HTTPRouteResponseHeaderModification` - 响应头修改
- `HTTPRouteBackendRequestHeaderModification` - 后端请求头修改
- `HTTPRouteHostRewrite` - Host 重写
- `HTTPRoutePathRewrite` - 路径重写
- `HTTPRouteRequestMirror` - 请求镜像
- `HTTPRouteRequestMultipleMirrors` - 多目标请求镜像
- `HTTPRouteRequestPercentageMirror` - 按比例请求镜像
- `HTTPRouteRequestTimeout` - 请求超时
- `HTTPRouteBackendTimeout` - 后端超时
- `HTTPRouteParentRefPort` - 父引用端口
- `HTTPRouteBackendProtocolH2C` - H2C 后端协议
- `HTTPRouteBackendProtocolWebSocket` - WebSocket 后端协议
- `HTTPRouteNamedRouteRule` - 命名路由规则
- `HTTPRouteCORS` - CORS 支持 (v1.5 新增)
- `HTTPRoutePortRedirect` - 端口重定向
- `HTTPRouteSchemeRedirect` - 协议重定向
- `HTTPRoutePathRedirect` - 路径重定向
- `HTTPRoute303RedirectStatusCode` - 303 重定向
- `HTTPRoute307RedirectStatusCode` - 307 重定向
- `HTTPRoute308RedirectStatusCode` - 308 重定向
- `HTTPRouteDestinationPortMatching` - 目标端口匹配 (experimental)

### TLSRoute Extended
- `TLSRouteModeTerminate` - TLS 终止模式
- `TLSRouteModeMixed` - TLS 混合模式 (experimental, v1.5 新增)

### GRPCRoute Extended
- `GRPCRouteNamedRouteRule` - gRPC 命名路由规则

## 数据来源

- 上游仓库: `kubernetes-sigs/gateway-api` tag `v1.5.0`
- 测试代码: `conformance/tests/`
- Conformance suite: `sigs.k8s.io/gateway-api/conformance v1.5.1`
