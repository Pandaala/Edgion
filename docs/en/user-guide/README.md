# User Guide

This section is intended for application developers and covers how to configure routing rules using the Gateway API.

## Table of Contents

### HTTPRoute
- [HTTPRoute Overview](./http-route/overview.md)
- Matches
  - [Path Matching](./http-route/matches/path.md)
  - [Header Matching](./http-route/matches/headers.md)
  - [Query Parameter Matching](./http-route/matches/query-params.md)
  - [HTTP Method Matching](./http-route/matches/method.md)
- Filters
  - [Filters Overview](./http-route/filters/overview.md)
  - [Plugin Composition and References 🔌](./http-route/filters/plugin-composition.md)
  - Gateway API Standard Filters
    - [RequestHeaderModifier](./http-route/filters/gateway-api/request-header-modifier.md)
    - [ResponseHeaderModifier](./http-route/filters/gateway-api/response-header-modifier.md)
    - [RequestRedirect](./http-route/filters/gateway-api/request-redirect.md)
    - [URLRewrite](./http-route/filters/gateway-api/url-rewrite.md)
  - Edgion Extension Plugins 🔌
    - [BasicAuth](./http-route/filters/edgion-plugins/basic-auth.md)
    - [CORS](./http-route/filters/edgion-plugins/cors.md)
    - [CSRF](./http-route/filters/edgion-plugins/csrf.md)
    - [IP Restriction](./http-route/filters/edgion-plugins/ip-restriction.md)
    - [JWT Auth](./http-route/filters/edgion-plugins/jwt-auth.md)
    - [Key Auth](./http-route/filters/edgion-plugins/key-auth.md)
    - [HMAC Auth](./http-route/filters/edgion-plugins/hmac-auth.md)
    - [Header Cert Auth](./http-route/filters/edgion-plugins/header-cert-auth.md)
    - [ProxyRewrite](./http-route/filters/edgion-plugins/proxy-rewrite.md)
    - [Request Restriction](./http-route/filters/edgion-plugins/request-restriction.md)
    - [Response Rewrite](./http-route/filters/edgion-plugins/response-rewrite.md)
    - [Rate Limiting (Single Instance)](./http-route/filters/edgion-plugins/rate-limit.md)
    - [Bandwidth Limit](./http-route/filters/edgion-plugins/bandwidth-limit.md)
    - [Request Mirror](./http-route/filters/edgion-plugins/request-mirror.md)
    - [Direct Endpoint](./http-route/filters/edgion-plugins/direct-endpoint.md)
    - [Dynamic Upstream](./http-route/filters/edgion-plugins/dynamic-upstream.md)
    - [Mock](./http-route/filters/edgion-plugins/mock.md)
    - [DSL](./http-route/filters/edgion-plugins/dsl.md)
- Backends
  - [Service Reference](./http-route/backends/service-ref.md)
  - [Weight Configuration](./http-route/backends/weight.md)
  - [Backend TLS](./http-route/backends/backend-tls.md)
  - [Backend Active Health Check 🔌](./http-route/backends/health-check.md)
- Resilience
  - [Timeout Configuration](./http-route/resilience/timeouts.md)
  - [Retry Policy](./http-route/resilience/retry.md)
  - [Session Persistence](./http-route/resilience/session-persistence.md)
- [Load Balancing Algorithms 🔌](./http-route/lb-algorithms.md)

### EdgionPlugins (Standalone Resources)

- [CtxSetter](./edgion-plugins/ctx-setter.md)
- [ForwardAuth](./edgion-plugins/forward-auth.md)
- [JWE Decrypt](./edgion-plugins/jwe-decrypt.md)
- [LDAP Auth](./edgion-plugins/ldap-auth.md)
- [OpenID Connect](./edgion-plugins/openid-connect.md)
- [RateLimit (Distributed)](./edgion-plugins/rate-limit.md)
- [RealIP](./edgion-plugins/real-ip.md)

### GRPCRoute

- [GRPCRoute Overview](./grpc-route/overview.md)
- [Match Rules](./grpc-route/matches/overview.md)
- [Filters](./grpc-route/filters/overview.md)
- [Backend Configuration](./grpc-route/backends/overview.md)

### TCPRoute

- [TCPRoute Overview](./tcp-route/overview.md)
- [Backend Configuration](./tcp-route/backends/overview.md)
- [Stream Plugins 🔌](./tcp-route/stream-plugins.md)

### UDPRoute

- [UDPRoute Overview](./udp-route/overview.md)
- [Backend Configuration](./udp-route/backends/overview.md)
- [Stream Plugins](./tcp-route/stream-plugins.md) (shared with TCPRoute)

### Advanced Features

- Canary Release: start with [Weight Configuration](./http-route/backends/weight.md)
- Blue-Green Deployment: start with [Weight Configuration](./http-route/backends/weight.md)
