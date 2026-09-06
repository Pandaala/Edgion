# Integration Suite Map

This file only answers three things:

1. Which `conf/` directory a given local integration test should live in
2. Where the corresponding Rust suite is
3. Whether `--gateway` is required

## Main entry-point mapping

| Family | YAML directory | Rust entry | `--gateway` | Notes |
|------|-----------|-----------|-------------|------|
| `EdgionBackend/SingleProvider` | `edgion-tests/integration/conf/EdgionBackend/SingleProvider/` | `SingleProviderTestSuite` | Yes | OpenAI-compatible proxying, H1/H2 streaming and downstream cancellation, credentials, usage/quota logs, and optional token metrics |
| `EdgionBackend/AnthropicProvider` | `edgion-tests/integration/conf/EdgionBackend/AnthropicProvider/` | `AnthropicProviderTestSuite` | Yes | Anthropic Messages request translation plus JSON and incremental SSE response translation |
| `EdgionBackend/Resilience` | `edgion-tests/integration/conf/EdgionBackend/Resilience/` | `ResilienceTestSuite` | Yes | Provider 429 Priority fallback, committed-SSE no-failover, half-open single-probe recovery |
| `EdgionBackend/CredentialExhaustion` | `edgion-tests/integration/conf/EdgionBackend/CredentialExhaustion/` | `CredentialExhaustionTestSuite` | Yes | Credential-first rotation then next service; RPM exhaust and TPM reconcile to local 503; needs Redis |
| `EdgionBackend/PolicyIntegration` | `edgion-tests/integration/conf/EdgionBackend/PolicyIntegration/DynamicTest/` | `AiBackendPolicyIntegration` dynamic scenario | Yes | AI backend custom CA, SNI/SAN, mTLS, Secret rotation, policy status, multi-target traffic policy publication, stable in-flight capture, hot reload, and stream termination boundaries; the Kubernetes runner supplies an isolated Gateway listener and in-cluster provider endpoints |
| `HTTPRoute/Basic` | `edgion-tests/integration/conf/HTTPRoute/Basic/` | `edgion-tests/src/client/suites/http_route/` | No | Direct HTTP basic capability |
| `HTTPRoute/SseStreaming` | `edgion-tests/integration/conf/HTTPRoute/SseStreaming/` | `SseStreamingTestSuite` | Yes | Paced SSE passthrough remains uncompressed while ordinary JSON is gzip-compressed |
| `HTTPRoute/Delegation` | `edgion-tests/integration/conf/HTTPRoute/Delegation/` | `DelegationTestSuite` | Yes | Parent→child delegation flatten: inherited filter, out-of-prefix drop, plain sibling |
| `HTTPRoute/Match` | `edgion-tests/integration/conf/HTTPRoute/Match/` | `HttpMatchTestSuite` | Yes | Relies on gateway route matching |
| `HTTPRoute/Backend/*` | `edgion-tests/integration/conf/HTTPRoute/Backend/` | `LBRoundRobinTestSuite`, `LBConsistentHashTestSuite`, `WeightedBackendTestSuite`, `TimeoutTestSuite`, `HealthCheckTestSuite`, `GrpcHealthCheckTestSuite`, `HealthCheckTransitionTestSuite`, `PartiallyInvalidTestSuite`, `RetryOnStatusTestSuite`, `CircuitBreakerTestSuite`, `SessionPersistenceTestSuite` | Yes | Load balancing, timeout, health check, partially invalid rules, retry, circuit breaking, session persistence |
| `HTTPRoute/Filters/*` | `edgion-tests/integration/conf/HTTPRoute/Filters/` | `HttpRedirectTestSuite`, `HttpSecurityTestSuite`, `HeaderModifierTestSuite`, `FilterConflictTestSuite` | Yes | Redirect, security, header modifier, filter conflict |
| `HTTPRoute/Protocol/WebSocket` | `edgion-tests/integration/conf/HTTPRoute/Protocol/WebSocket/` | `WebSocketTestSuite` | No | WebSocket backend validation |
| `GRPCRoute/Basic` | `edgion-tests/integration/conf/GRPCRoute/Basic/` | `GrpcTestSuite` | No | gRPC direct basics |
| `GRPCRoute/Match` | `edgion-tests/integration/conf/GRPCRoute/Match/` | `GrpcMatchTestSuite` | Yes | gRPC route matching (Exact + RegularExpression) |
| `GRPCRoute/Backend/SessionPersistence` | `edgion-tests/integration/conf/GRPCRoute/Backend/SessionPersistence/` | `GrpcSessionPersistenceTestSuite` | Yes | Cookie/Header session persistence, stale-token fallback, on Service backed by per-port EndpointSlices |
| `TCPRoute/Basic` | `edgion-tests/integration/conf/TCPRoute/Basic/` | `TcpTestSuite` | No | TCP echo |
| `TCPRoute/DeniedBackendRef` | `edgion-tests/integration/conf/TCPRoute/DeniedBackendRef/` | `TcpDeniedBackendRefTestSuite` | Yes | Cross-ns TCP backendRef without ReferenceGrant fail-closes before echo |
| `TCPRoute/StreamPlugins` | `edgion-tests/integration/conf/TCPRoute/StreamPlugins/` | `TcpStreamPluginsTestSuite` | Yes | Stream plugin wiring on TCP |
| `TLSRoute/Basic` | `edgion-tests/integration/conf/TLSRoute/Basic/` | `TlsRouteTestSuite` | Yes | SNI/TLS route basics |
| `TLSRoute/ProxyProtocol` | `edgion-tests/integration/conf/TLSRoute/ProxyProtocol/` | `TlsProxyProtocolTestSuite` | Yes | Depends on TCP PP2 test backend |
| `TLSRoute/StreamPlugins` | `edgion-tests/integration/conf/TLSRoute/StreamPlugins/` | `TlsStreamPluginsTestSuite` | Yes | Stream plugin on TLS route |
| `TLSRoute/MultiSNI` | `edgion-tests/integration/conf/TLSRoute/MultiSNI/` | `TlsMultiSniTestSuite` | Yes | Multi-hostname / multi-TLSRoute routing |
| `TLSRoute/BothAbsentParentRef` | `edgion-tests/integration/conf/TLSRoute/BothAbsentParentRef/` | `TlsBothAbsentParentRefTestSuite` | Yes | parentRef missing/fallback validation |
| `TLSRoute/Passthrough` | `edgion-tests/integration/conf/TLSRoute/Passthrough/` | `TlsPassthroughTestSuite` | Yes | TLS passthrough mode |
| `TLSRoute/DeniedBackendRef` | `edgion-tests/integration/conf/TLSRoute/DeniedBackendRef/` | `TlsDeniedBackendRefTestSuite` | Yes | Cross-ns TLS terminate and passthrough backendRefs fail-close before dial |
| `UDPRoute/Basic` | `edgion-tests/integration/conf/UDPRoute/Basic/` | `UdpTestSuite` | No | UDP echo |
| `UDPRoute/DeniedBackendRef` | `edgion-tests/integration/conf/UDPRoute/DeniedBackendRef/` | `UdpDeniedBackendRefTestSuite` | Yes | Cross-ns UDP backendRef without ReferenceGrant drops the datagram |
| `Gateway/Security` | `edgion-tests/integration/conf/Gateway/Security/` | `SecurityTestSuite` | Yes | Gateway security capability |
| `Gateway/RealIP` | `edgion-tests/integration/conf/Gateway/RealIP/` | `RealIpTestSuite` | Yes | real-ip annotation and header parsing |
| `Gateway/AllowedRoutes/*` | `edgion-tests/integration/conf/Gateway/AllowedRoutes/` | `AllowedRoutes*TestSuite` | Yes | namespace / kinds / selector |
| `Gateway/TLS/*` | `edgion-tests/integration/conf/Gateway/TLS/` | `BackendTlsTestSuite`, `BackendTlsSanTestSuite`, `GatewayTlsTestSuite`, `GatewayTlsNoHostnameListenerTestSuite` | Yes | Backend TLS, backend TLS SAN validation, listener TLS |
| `Gateway/FrontendValidation` | `edgion-tests/integration/conf/Gateway/FrontendValidation/` | `FrontendValidationTestSuite` | Yes | Frontend mTLS client cert validation |
| `Gateway/DynamicTest` | `edgion-tests/integration/conf/Gateway/DynamicTest/` | `InitialPhaseTestSuite`, `UpdatePhaseTestSuite` | Yes | Config dynamic update, orchestrated by the `GatewayDynamic` scenario (see [the dynamic-testing section in 01-integration-testing.md](../01-integration-testing.md)) |
| `Gateway/ListenerHostname` | `edgion-tests/integration/conf/Gateway/ListenerHostname/` | `ListenerHostnameTestSuite` | Yes | listener hostname selection |
| `Gateway/PortConflict` | `edgion-tests/integration/conf/Gateway/PortConflict/` | `PortConflictTestSuite` | Yes | listener port conflict |
| `Gateway/StreamPlugins` | `edgion-tests/integration/conf/Gateway/StreamPlugins/` | `StreamPluginsTestSuite` | Yes | Gateway-level stream plugin |
| `Gateway/Combined` | `edgion-tests/integration/conf/Gateway/Combined/` | `CombinedScenariosTestSuite` | Yes | Combined scenarios |
| `EdgionPlugins/*` | `edgion-tests/integration/conf/EdgionPlugins/` | `edgion-tests/src/client/suites/edgion_plugins/` | Yes | Most plugin tests share the `EdgionPlugins` port key |
| `EdgionTls/*` | `edgion-tests/integration/conf/EdgionTls/` | `edgion-tests/src/client/suites/edgion_tls/` | Yes | HTTPS, gRPC TLS, mTLS, cipher, port-only |
| `Status/RefGrant` | `edgion-tests/integration/conf/Status/RefGrant/` | `RefGrantStatusTestSuite` | No | `ReferenceGrant` status validation |
| `Status/BackendTLSPolicy` | `edgion-tests/integration/conf/Status/BackendTLSPolicy/` | `BackendTlsStatusTestSuite` | No | `BackendTLSPolicy` validation and status |
| `Status/TLSListenerMode` | `edgion-tests/integration/conf/Status/TLSListenerMode/` | `TlsListenerModeStatusTestSuite` | Yes | TLS listener mode status validation (Passthrough=Accepted, Mixed=UnsupportedTLSMode) |
| `Services/acme` | `edgion-tests/integration/conf/Services/acme/` | `AcmeTestSuite` | Depends on the scenario | Service-level integration |

`LinkSys` tests live under `edgion-tests/integration/conf/LinkSys/` too, but they have an independent bash + Docker Compose pipeline; see [../03-link-sys-testing.md](../03-link-sys-testing.md) first.

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
./edgion-tests/integration/scripts/integration/run_integration.sh -r HTTPRoute

# Run an item under a family
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r TLSRoute -i MultiSNI

# Run a plugin item
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i JwtAuth

# Replay directly after preserving the scene
./target/debug/test_client -g -r Gateway -i StreamPlugins
```
