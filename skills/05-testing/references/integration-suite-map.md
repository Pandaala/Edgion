# Integration Suite Map

This file only answers three things:

1. Which `conf/` directory a given local integration test should live in
2. Where the corresponding Rust suite is
3. Whether `--gateway` is required

## Main entry-point mapping

| Family | YAML directory | Rust entry | `--gateway` | Notes |
|------|-----------|-----------|-------------|------|
| `HTTPRoute/Basic` | `edgion-tests/conf/HTTPRoute/Basic/` | `edgion-tests/src/client/suites/http_route/` | No | Direct HTTP basic capability |
| `HTTPRoute/Match` | `edgion-tests/conf/HTTPRoute/Match/` | `HttpMatchTestSuite` | Yes | Relies on gateway route matching |
| `HTTPRoute/Backend/*` | `edgion-tests/conf/HTTPRoute/Backend/` | `LBRoundRobinTestSuite`, `LBConsistentHashTestSuite`, `WeightedBackendTestSuite`, `TimeoutTestSuite`, `HealthCheckTestSuite`, `GrpcHealthCheckTestSuite`, `HealthCheckTransitionTestSuite`, `PartiallyInvalidTestSuite`, `RetryOnStatusTestSuite`, `CircuitBreakerTestSuite`, `SessionPersistenceTestSuite` | Yes | Load balancing, timeout, health check, partially invalid rules, retry, circuit breaking, session persistence |
| `HTTPRoute/Filters/*` | `edgion-tests/conf/HTTPRoute/Filters/` | `HttpRedirectTestSuite`, `HttpSecurityTestSuite`, `HeaderModifierTestSuite`, `FilterConflictTestSuite` | Yes | Redirect, security, header modifier, filter conflict |
| `HTTPRoute/Protocol/WebSocket` | `edgion-tests/conf/HTTPRoute/Protocol/WebSocket/` | `WebSocketTestSuite` | No | WebSocket backend validation |
| `GRPCRoute/Basic` | `edgion-tests/conf/GRPCRoute/Basic/` | `GrpcTestSuite` | No | gRPC direct basics |
| `GRPCRoute/Match` | `edgion-tests/conf/GRPCRoute/Match/` | `GrpcMatchTestSuite` | Yes | gRPC route matching (Exact + RegularExpression) |
| `TCPRoute/Basic` | `edgion-tests/conf/TCPRoute/Basic/` | `TcpTestSuite` | No | TCP echo |
| `TCPRoute/StreamPlugins` | `edgion-tests/conf/TCPRoute/StreamPlugins/` | `TcpStreamPluginsTestSuite` | Yes | Stream plugin wiring on TCP |
| `TLSRoute/Basic` | `edgion-tests/conf/TLSRoute/Basic/` | `TlsRouteTestSuite` | Yes | SNI/TLS route basics |
| `TLSRoute/ProxyProtocol` | `edgion-tests/conf/TLSRoute/ProxyProtocol/` | `TlsProxyProtocolTestSuite` | Yes | Depends on TCP PP2 test backend |
| `TLSRoute/StreamPlugins` | `edgion-tests/conf/TLSRoute/StreamPlugins/` | `TlsStreamPluginsTestSuite` | Yes | Stream plugin on TLS route |
| `TLSRoute/MultiSNI` | `edgion-tests/conf/TLSRoute/MultiSNI/` | `TlsMultiSniTestSuite` | Yes | Multi-hostname / multi-TLSRoute routing |
| `TLSRoute/BothAbsentParentRef` | `edgion-tests/conf/TLSRoute/BothAbsentParentRef/` | `TlsBothAbsentParentRefTestSuite` | Yes | parentRef missing/fallback validation |
| `TLSRoute/Passthrough` | `edgion-tests/conf/TLSRoute/Passthrough/` | `TlsPassthroughTestSuite` | Yes | TLS passthrough mode |
| `UDPRoute/Basic` | `edgion-tests/conf/UDPRoute/Basic/` | `UdpTestSuite` | No | UDP echo |
| `Gateway/Security` | `edgion-tests/conf/Gateway/Security/` | `SecurityTestSuite` | Yes | Gateway security capability |
| `Gateway/RealIP` | `edgion-tests/conf/Gateway/RealIP/` | `RealIpTestSuite` | Yes | real-ip annotation and header parsing |
| `Gateway/AllowedRoutes/*` | `edgion-tests/conf/Gateway/AllowedRoutes/` | `AllowedRoutes*TestSuite` | Yes | namespace / kinds / selector |
| `Gateway/TLS/*` | `edgion-tests/conf/Gateway/TLS/` | `BackendTlsTestSuite`, `BackendTlsSanTestSuite`, `GatewayTlsTestSuite`, `GatewayTlsNoHostnameListenerTestSuite` | Yes | Backend TLS, backend TLS SAN validation, listener TLS |
| `Gateway/FrontendValidation` | `edgion-tests/conf/Gateway/FrontendValidation/` | `FrontendValidationTestSuite` | Yes | Frontend mTLS client cert validation |
| `Gateway/DynamicTest` | `edgion-tests/conf/Gateway/DynamicTest/` | `InitialPhaseTestSuite`, `UpdatePhaseTestSuite` | Yes | Config dynamic update, orchestrated by the `GatewayDynamic` scenario (see [the dynamic-testing section in 01-integration-testing.md](../01-integration-testing.md)) |
| `Gateway/ListenerHostname` | `edgion-tests/conf/Gateway/ListenerHostname/` | `ListenerHostnameTestSuite` | Yes | listener hostname selection |
| `Gateway/PortConflict` | `edgion-tests/conf/Gateway/PortConflict/` | `PortConflictTestSuite` | Yes | listener port conflict |
| `Gateway/StreamPlugins` | `edgion-tests/conf/Gateway/StreamPlugins/` | `StreamPluginsTestSuite` | Yes | Gateway-level stream plugin |
| `Gateway/Combined` | `edgion-tests/conf/Gateway/Combined/` | `CombinedScenariosTestSuite` | Yes | Combined scenarios |
| `EdgionPlugins/*` | `edgion-tests/conf/EdgionPlugins/` | `edgion-tests/src/client/suites/edgion_plugins/` | Yes | Most plugin tests share the `EdgionPlugins` port key |
| `EdgionTls/*` | `edgion-tests/conf/EdgionTls/` | `edgion-tests/src/client/suites/edgion_tls/` | Yes | HTTPS, gRPC TLS, mTLS, cipher, port-only |
| `Status/RefGrant` | `edgion-tests/conf/Status/RefGrant/` | `RefGrantStatusTestSuite` | No | `ReferenceGrant` status validation |
| `Status/BackendTLSPolicy` | `edgion-tests/conf/Status/BackendTLSPolicy/` | `BackendTlsStatusTestSuite` | No | `BackendTLSPolicy` validation and status |
| `Status/TLSListenerMode` | `edgion-tests/conf/Status/TLSListenerMode/` | `TlsListenerModeStatusTestSuite` | Yes | TLS listener mode status validation (Passthrough=Accepted, Mixed=UnsupportedTLSMode) |
| `Services/acme` | `edgion-tests/conf/Services/acme/` | `AcmeTestSuite` | Depends on the scenario | Service-level integration |

`LinkSys` tests live under `edgion-tests/conf/LinkSys/` too, but they have an independent bash + Docker Compose pipeline; see [../03-link-sys-testing.md](../03-link-sys-testing.md) first.

## Common `EdgionPlugins` items

These directories all use the same `EdgionPlugins` listener key, only the suite assertions differ:

- `BasicAuth`
- `Cors`
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
- `RegionRoute`
- `DirectEndpoint`
- `Dsl`

When adding a new HTTP plugin test, first look for the closest existing implementation under `edgion-tests/src/client/suites/edgion_plugins/` before deciding whether to add a new subdirectory.

## Top 3 things easy to miss in `test_client.rs`

When adding a new item or suite family, check first:

1. Whether `resolve_suite()` maps the CLI argument to the correct `<Resource>/<Item>`
2. Whether `suite_to_port_key()` maps it to the correct `ports.json` key
3. Whether `add_suites_for_suite()` actually adds the suite instance to the runner

A lot of "YAML is there but the test didn't run" issues end up at one of these three spots.

## Common command templates

```bash
# Run a family
./edgion-tests/scripts/integration/run_integration.sh -r HTTPRoute

# Run an item under a family
./edgion-tests/scripts/integration/run_integration.sh --no-prepare -r TLSRoute -i MultiSNI

# Run a plugin item
./edgion-tests/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i JwtAuth

# Replay directly after preserving the scene
./target/debug/test_client -g -r Gateway -i StreamPlugins
```
