# Edgion

A high-performance API Gateway built on [Pingora](https://github.com/cloudflare/pingora) and [Gateway API](https://gateway-api.sigs.k8s.io/). Designed for both Kubernetes and standalone (bare-metal / VM) environments.

[![Rust](https://img.shields.io/badge/rust-1.75%2B-blue.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

## Features

- **High Performance** — Built on Cloudflare's Pingora framework with Rust; clear stack traces and better debugging compared to Lua/Go-based proxies
- **Gateway API v1** — Full support for the standard Kubernetes [Gateway API](https://gateway-api.sigs.k8s.io/) (v1.4.0), including [HTTPRoute](docs/en/user-guide/http-route/overview.md), [GRPCRoute](docs/en/user-guide/grpc-route/overview.md), [TCPRoute](docs/en/user-guide/tcp-route/overview.md), [UDPRoute](docs/en/user-guide/udp-route/overview.md), and [TLSRoute](docs/en/ops-guide/gateway/tls/tls-termination.md)
- **Dual Deployment** — Runs in Kubernetes (watching CRDs) or standalone mode (local YAML config), same binary
- **Hot Reload** — Routes, backends, plugins, and TLS certificates all take effect immediately without restart or connection drops
- **Flexible Routing** — [Path](docs/en/user-guide/http-route/matches/path.md) (Exact / Prefix / Regex), [Header](docs/en/user-guide/http-route/matches/headers.md), [Query Parameter](docs/en/user-guide/http-route/matches/query-params.md), and [Method](docs/en/user-guide/http-route/matches/method.md) matching with priority-based rule evaluation; supports domain-level routing with exact and wildcard hostnames
- **Multi-Protocol** — HTTP/1.1, HTTP/2, gRPC, TCP, UDP, WebSocket, and SNI proxy
- **Advanced TLS** — [mTLS](docs/en/ops-guide/infrastructure/mtls.md), dynamic certificate loading, [ACME](docs/en/ops-guide/gateway/tls/acme.md) auto-provisioning (HTTP-01 / DNS-01), and [per-domain TLS policy](docs/en/ops-guide/gateway/tls/edgion-tls.md)
  - **Per-Domain TLS Version & Cipher Control** — Each domain can independently configure minimum TLS version and allowed cipher suites via [EdgionTls](docs/en/ops-guide/gateway/tls/edgion-tls.md), enabling legacy algorithm compatibility for specific domains while enforcing strict security on others
- **Extensible [Plugin System](docs/en/user-guide/http-route/filters/overview.md)** — 25+ built-in HTTP plugins and TCP/UDP stream plugins with [plugin composition](docs/en/user-guide/http-route/filters/plugin-composition.md) support, see [full list](#plugins) below
- **Resilience** — [Retry](docs/en/user-guide/http-route/resilience/retry.md) with configurable backoff, [timeouts](docs/en/user-guide/http-route/resilience/timeouts.md), and [session persistence](docs/en/user-guide/http-route/resilience/session-persistence.md) (Cookie / Header)
- **Load Balancing** — [Multiple algorithms](docs/en/user-guide/http-route/lb-algorithms.md) including Round Robin, EWMA, Least Connections, Consistent Hashing, and Weighted selection with [active health checks](docs/en/user-guide/http-route/backends/health-check.md)
- **Observability** — [Unified access log](docs/en/ops-guide/observability/access-log.md) captures the full request lifecycle (routing, plugins, backend, errors) in a single JSON line; [Prometheus metrics](docs/en/ops-guide/observability/metrics.md) endpoint with distributed tracing readiness
- **Sandbox Gateway** — Isolated gateway environments for controlled execution

## Plugins

Edgion provides a rich set of built-in plugins via the [EdgionPlugins](docs/en/user-guide/http-route/filters/overview.md) CRD. Plugins can be attached to routes through Gateway API's `ExtensionRef` filter or composed via [Plugin Composition](docs/en/user-guide/http-route/filters/plugin-composition.md).

### Authentication

| Plugin | Description |
|--------|-------------|
| [Basic Auth](docs/en/user-guide/http-route/filters/edgion-plugins/basic-auth.md) | HTTP Basic authentication with htpasswd support |
| [JWT Auth](docs/en/user-guide/http-route/filters/edgion-plugins/jwt-auth.md) | JSON Web Token validation and claim extraction (HS256 / RS256 / ES256) |
| [Key Auth](docs/en/user-guide/http-route/filters/edgion-plugins/key-auth.md) | API key authentication via header or query param |
| [HMAC Auth](docs/en/user-guide/http-route/filters/edgion-plugins/hmac-auth.md) | HMAC-based request signing verification |
| [LDAP Auth](docs/en/user-guide/edgion-plugins/ldap-auth.md) | LDAP / Active Directory authentication |
| [Forward Auth](docs/en/user-guide/edgion-plugins/forward-auth.md) | Delegate authentication to an external service |
| [OpenID Connect](docs/en/user-guide/edgion-plugins/openid-connect.md) | OIDC-based SSO with Code Flow + PKCE, session cookie, and token refresh |
| [JWE Decrypt](docs/en/user-guide/edgion-plugins/jwe-decrypt.md) | JSON Web Encryption decryption with payload-to-header mapping |
| [Header Cert Auth](docs/en/user-guide/http-route/filters/edgion-plugins/header-cert-auth.md) | Client certificate header-based authentication |

### Security

| Plugin | Description |
|--------|-------------|
| [CORS](docs/en/user-guide/http-route/filters/edgion-plugins/cors.md) | Cross-Origin Resource Sharing policy |
| [CSRF](docs/en/user-guide/http-route/filters/edgion-plugins/csrf.md) | Cross-Site Request Forgery protection |
| [IP Restriction](docs/en/user-guide/http-route/filters/edgion-plugins/ip-restriction.md) | IP allowlist / denylist access control |
| [Request Restriction](docs/en/user-guide/http-route/filters/edgion-plugins/request-restriction.md) | Attribute-based access control on headers, cookies, query params, path, method, and referer |

### Traffic Management

| Plugin | Description |
|--------|-------------|
| [Rate Limit](docs/en/user-guide/http-route/filters/edgion-plugins/rate-limit.md) | Local rate limiting with CMS algorithm, multi-dimension keys (IP / Header / Path) |
| [Rate Limit (Redis)](docs/en/user-guide/edgion-plugins/rate-limit.md) | Distributed rate limiting backed by Redis with cluster-wide coordination |
| [Proxy Rewrite](docs/en/user-guide/http-route/filters/edgion-plugins/proxy-rewrite.md) | Rewrite upstream URI, host, method, and headers with regex and variable support |
| [Response Rewrite](docs/en/user-guide/http-route/filters/edgion-plugins/response-rewrite.md) | Modify response status, headers (set / add / remove / rename), and body |
| [Bandwidth Limit](docs/en/user-guide/http-route/filters/edgion-plugins/bandwidth-limit.md) | Per-route downstream bandwidth throttling |
| [Request Mirror](docs/en/user-guide/http-route/filters/edgion-plugins/request-mirror.md) | Mirror traffic to a secondary backend |
| [Direct Endpoint](docs/en/user-guide/http-route/filters/edgion-plugins/direct-endpoint.md) | Route to a specific endpoint bypassing service discovery |
| [Dynamic Upstream](docs/en/user-guide/http-route/filters/edgion-plugins/dynamic-upstream.md) | Dynamically resolve upstream targets at request time |

### Observability & Utilities

| Plugin | Description |
|--------|-------------|
| [Real IP](docs/en/user-guide/edgion-plugins/real-ip.md) | Extract real client IP from proxy headers with recursive trusted-IP resolution |
| [Ctx Setter](docs/en/user-guide/edgion-plugins/ctx-setter.md) | Set context variables for downstream plugins, conditions, and access log |
| [Mock](docs/en/user-guide/http-route/filters/edgion-plugins/mock.md) | Return mock responses for testing |
| [DSL](docs/en/user-guide/http-route/filters/edgion-plugins/dsl.md) | Custom request processing via built-in DSL expressions |

### Gateway API Standard Filters

These filters are built-in to the Gateway API specification and require no CRD:

[Request Header Modifier](docs/en/user-guide/http-route/filters/gateway-api/request-header-modifier.md) | [Response Header Modifier](docs/en/user-guide/http-route/filters/gateway-api/response-header-modifier.md) | [Request Redirect](docs/en/user-guide/http-route/filters/gateway-api/request-redirect.md) | [URL Rewrite](docs/en/user-guide/http-route/filters/gateway-api/url-rewrite.md)

### Stream Plugins (TCP/UDP)

| Plugin | Description |
|--------|-------------|
| [IP Restriction](docs/en/user-guide/tcp-route/stream-plugins.md) | Connection-level IP allowlist / denylist |

## Documentation

- 🇨🇳 [Chinese Documentation](docs/zh-CN/README.md)
- 🇬🇧 [English Documentation](docs/en/README.md)

## Getting Started

For setup and usage details, see the [User Guide](docs/en/user-guide/README.md) and the [examples](examples/README.md).

## Testing

```bash
# Run unit tests
cargo test --all --tests

# Run integration tests
cd examples/testing
./run_integration_test.sh
```

## Architecture

Edgion follows a **Controller–Gateway** separation architecture connected via gRPC:

- **Controller** (`edgion-controller`) — Watches configuration sources (Kubernetes CRDs or local YAML), validates and pre-parses resources, then streams them to gateways via gRPC. Handles ACME certificates and status updates.
- **Gateway** (`edgion-gateway`) — Stateless data plane built on Pingora. Receives configuration from the controller, executes routing, plugin chains, load balancing, TLS termination, and access logging.
- **CLI** (`edgion-ctl`) — Management tool for inspecting and operating both controller and gateway.

```
                ┌──────────────┐
                │  K8s API /   │
                │  Local YAML  │
                └──────┬───────┘
                       │ watch
                ┌──────▼───────┐
                │  Controller  │ ── Admin API :5800
                └──────┬───────┘
                       │ gRPC :5810
          ┌────────────┼────────────┐
          │            │            │
   ┌──────▼──┐  ┌──────▼──┐  ┌──────▼──┐
   │ Gateway │  │ Gateway │  │ Gateway │
   │  :80/443│  │  :80/443│  │  :80/443│
   └─────────┘  └─────────┘  └─────────┘
```

For architecture details, see [Architecture Overview](docs/en/dev-guide/architecture-overview.md).

## Roadmap

- [ ] **Gateway API Conformance Testing** — Broader validation against Gateway API conformance suites
- [ ] **Cache Plugin** — Response caching and cache-anything support
- [ ] **Async MQ / Log Gateway** — Asynchronous message queue integration
- [ ] **Full-Chain Reconcile** — End-to-end configuration reconciliation
- [ ] **HTTP/3** — Native HTTP/3 support across the gateway stack
- [ ] **AI Gateway** — AI-specific plugins and policies
- [ ] **AI Mesh** — AI service mesh capabilities
- [ ] **MCP Proxy** — Model Context Protocol proxy
- [ ] **AI Workflow Engine** — AI-powered workflow orchestration

## A Note

Please excuse the many commits with minimal detail. AI-assisted development is moving faster than I can document every change thoroughly right now.

## Contributing

Contributions are welcome! Please check out:

- [Developer Documentation](docs/en/dev-guide/README.md)
- [Adding New Resources Guide](docs/en/dev-guide/add-new-resource-guide.md)

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## Acknowledgments

- [Pingora](https://github.com/cloudflare/pingora) — High-performance proxy framework by Cloudflare
- [Gateway API](https://gateway-api.sigs.k8s.io/) — Kubernetes SIG Network
- [kube-rs](https://kube.rs/) — Kubernetes client library for Rust
- [nom](https://github.com/rust-bakery/nom) — Parser combinator framework for Rust

---

**Version**: v0.1.1  
**Last Updated**: 2026-03-08
