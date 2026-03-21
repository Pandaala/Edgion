# Integration Suite Map

这个文件只负责回答三件事：

1. 某个本地集成测试应该放在哪个 `conf/` 目录
2. 对应的 Rust suite 在哪
3. 是否必须走 `--gateway`

## 主入口映射

| 家族 | YAML 目录 | Rust 入口 | `--gateway` | 说明 |
|------|-----------|-----------|-------------|------|
| `HTTPRoute/Basic` | `examples/test/conf/HTTPRoute/Basic/` | `examples/code/client/suites/http_route/` | 否 | 直连 HTTP 基础能力 |
| `HTTPRoute/Match` | `examples/test/conf/HTTPRoute/Match/` | `HttpMatchTestSuite` | 是 | 依赖 gateway 路由匹配 |
| `HTTPRoute/Backend/*` | `examples/test/conf/HTTPRoute/Backend/` | `LBRoundRobinTestSuite`、`LBConsistentHashTestSuite`、`WeightedBackendTestSuite`、`TimeoutTestSuite`、`HealthCheckTestSuite`、`HealthCheckTransitionTestSuite` | 是 | 负载均衡、超时、健康检查 |
| `HTTPRoute/Filters/*` | `examples/test/conf/HTTPRoute/Filters/` | `HttpRedirectTestSuite`、`HttpSecurityTestSuite`、`HeaderModifierTestSuite` | 是 | redirect、安全、header modifier |
| `HTTPRoute/Protocol/WebSocket` | `examples/test/conf/HTTPRoute/Protocol/WebSocket/` | `WebSocketTestSuite` | 否 | WebSocket 后端验证 |
| `GRPCRoute/Basic` | `examples/test/conf/GRPCRoute/Basic/` | `GrpcTestSuite` | 否 | gRPC 直连基础 |
| `GRPCRoute/Match` | `examples/test/conf/GRPCRoute/Match/` | `GrpcMatchTestSuite` | 是 | gRPC 路由匹配 |
| `TCPRoute/Basic` | `examples/test/conf/TCPRoute/Basic/` | `TcpTestSuite` | 否 | TCP echo |
| `TCPRoute/StreamPlugins` | `examples/test/conf/TCPRoute/StreamPlugins/` | `TcpStreamPluginsTestSuite` | 是 | stream plugin 在 TCP 上的接线 |
| `TLSRoute/Basic` | `examples/test/conf/TLSRoute/Basic/` | `TlsRouteTestSuite` | 是 | SNI/TLS route 基础 |
| `TLSRoute/ProxyProtocol` | `examples/test/conf/TLSRoute/ProxyProtocol/` | `TlsProxyProtocolTestSuite` | 是 | 依赖 TCP PP2 测试后端 |
| `TLSRoute/StreamPlugins` | `examples/test/conf/TLSRoute/StreamPlugins/` | `TlsStreamPluginsTestSuite` | 是 | TLS route 上的 stream plugin |
| `TLSRoute/MultiSNI` | `examples/test/conf/TLSRoute/MultiSNI/` | `TlsMultiSniTestSuite` | 是 | 多 hostname / 多 TLSRoute 路由 |
| `TLSRoute/BothAbsentParentRef` | `examples/test/conf/TLSRoute/BothAbsentParentRef/` | `TlsBothAbsentParentRefTestSuite` | 是 | parentRef 缺失/回退验证 |
| `UDPRoute/Basic` | `examples/test/conf/UDPRoute/Basic/` | `UdpTestSuite` | 否 | UDP echo |
| `Gateway/Security` | `examples/test/conf/Gateway/Security/` | `SecurityTestSuite` | 是 | gateway 安全能力 |
| `Gateway/RealIP` | `examples/test/conf/Gateway/RealIP/` | `RealIpTestSuite` | 是 | real-ip 注解和头部解析 |
| `Gateway/AllowedRoutes/*` | `examples/test/conf/Gateway/AllowedRoutes/` | `AllowedRoutes*TestSuite` | 是 | namespace / kinds / selector |
| `Gateway/TLS/*` | `examples/test/conf/Gateway/TLS/` | `BackendTlsTestSuite`、`GatewayTlsTestSuite`、`GatewayTlsNoHostnameListenerTestSuite` | 是 | backend TLS、listener TLS |
| `Gateway/DynamicTest` | `examples/test/conf/Gateway/DynamicTest/` | `InitialPhaseTestSuite`、`UpdatePhaseTestSuite` | 是 | 配置动态更新，注意 `--phase` |
| `Gateway/ListenerHostname` | `examples/test/conf/Gateway/ListenerHostname/` | `ListenerHostnameTestSuite` | 是 | listener hostname 选择 |
| `Gateway/PortConflict` | `examples/test/conf/Gateway/PortConflict/` | `PortConflictTestSuite` | 是 | listener 端口冲突 |
| `Gateway/StreamPlugins` | `examples/test/conf/Gateway/StreamPlugins/` | `StreamPluginsTestSuite` | 是 | gateway 级 stream plugin |
| `Gateway/Combined` | `examples/test/conf/Gateway/Combined/` | `CombinedScenariosTestSuite` | 是 | 组合场景 |
| `EdgionPlugins/*` | `examples/test/conf/EdgionPlugins/` | `examples/code/client/suites/edgion_plugins/` | 是 | 绝大多数插件测试共用 `EdgionPlugins` 端口 key |
| `EdgionTls/*` | `examples/test/conf/EdgionTls/` | `examples/code/client/suites/edgion_tls/` | 是 | HTTPS、gRPC TLS、mTLS、cipher、port-only |
| `ref-grant-status` | `examples/test/conf/ref-grant-status/` | `RefGrantStatusTestSuite` | 否 | `ReferenceGrant` 状态验证 |
| `Services/acme` | `examples/test/conf/Services/acme/` | `AcmeTestSuite` | 视场景而定 | 服务级联调 |

`LinkSys` 测试虽然也在 `examples/test/conf/LinkSys/` 下，但它有独立的 bash + Docker Compose 链路，优先看 [../03-link-sys-testing.md](../03-link-sys-testing.md)。

## `EdgionPlugins` 常见 item

这些目录通常都走同一个 `EdgionPlugins` listener key，只是 suite 断言不同：

- `BasicAuth`
- `JwtAuth`
- `JweDecrypt`
- `KeyAuth`
- `HmacAuth`
- `HeaderCertAuth`
- `LdapAuth`
- `ForwardAuth`
- `OpenidConnect`
- `WebhookKeyGet`
- `CtxSet`
- `PluginCondition`
- `DebugAccessLog`
- `ProxyRewrite`
- `RateLimit`
- `BandwidthLimit`
- `RealIp`
- `RequestMirror`
- `RequestRestriction`
- `ResponseRewrite`
- `DynamicInternalUpstream`
- `DynamicExternalUpstream`
- `DirectEndpoint`
- `Dsl`

如果新增一个 HTTP 插件测试，优先在 `examples/code/client/suites/edgion_plugins/` 下找最近似的已有实现，再决定要不要新增子目录。

## `test_client.rs` 里最容易漏的 3 个点

当你新增一个新的 item 或 suite family 时，优先检查：

1. `resolve_suite()` 是否能把 CLI 参数映射到正确的 `<Resource>/<Item>`
2. `suite_to_port_key()` 是否把它映射到正确的 `ports.json` key
3. `add_suites_for_suite()` 是否真正把 suite 实例加进 runner

很多“YAML 都在，但测试怎么没命中”的问题，最后都落在这三处之一。

## 常用命令模板

```bash
# 跑 family
./examples/test/scripts/integration/run_integration.sh -r HTTPRoute

# 跑 family 下某个 item
./examples/test/scripts/integration/run_integration.sh --no-prepare -r TLSRoute -i MultiSNI

# 跑插件类 item
./examples/test/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i JwtAuth

# 保留现场后直接重放
./target/debug/examples/test_client -g -r Gateway -i StreamPlugins
```
