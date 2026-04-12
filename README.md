# Edgion

[中文](docs/zh-CN/README.md) | English

## What is Edgion?

A high-performance API Gateway built on [Pingora](https://github.com/cloudflare/pingora) and [Gateway API](https://gateway-api.sigs.k8s.io/). Designed for both Kubernetes and standalone (bare-metal / VM) environments. A modern Rust-native alternative to Kong, APISIX, Traefik, Envoy, and HAProxy.

## Feature Highlights

- **Rust-native performance** — Built on Cloudflare's Pingora and Tokio. Memory-safe, no GC pauses, predictable latency under load.
- **Gateway API support** — Native implementation of the Kubernetes Gateway API standard. Runs in Kubernetes or standalone mode from the same binary.
  - `v1` GA: GatewayClass · Gateway · HTTPRoute · GRPCRoute · TLSRoute · ReferenceGrant
  - `v1alpha2` Experimental: TCPRoute · UDPRoute
- **Multi-protocol** — HTTP/1.1, HTTP/2, gRPC, TCP, UDP, WebSocket, SNI proxy.
- **Dynamic hot reload** — Every change to routes, backends, plugins, or certificates propagates across all gateways instantly. No restart.
- **Production-ready Kubernetes controller** — Full reconciliation loop, status reporting, and CRD lifecycle management out of the box.
- **High-performance routing** — Radix tree with cache optimization and path variable support for Exact/Prefix matching. Priority-based rule evaluation per Gateway API spec.
- **Advanced TLS** — mTLS, ACME auto-provisioning (HTTP-01 / DNS-01), per-domain TLS version and cipher suite control.
- **Powerful plugin system** — Composable via plugin reference, with a built-in DSL for custom logic.
  - HTTP plugins: Authentication · Security · Traffic management · Observability (25+)
  - Stream plugins: TCP/UDP traffic control
  - DSL: OpenResty/Lua-level expressiveness with native Rust performance
- **Deep observability** — Structured logs across the full request lifecycle. One line, full picture. AI friendly.
  - Access log: routing/upstream details and per-plugin behaviors in a single JSON line
  - TLS log: SSL handshake and certificate details per connection
  - Stream log: connection lifecycle events for long-lived connections (WebSocket, TCP)
  - Flexible output: local file or remote Kafka / Elasticsearch
- **Kubernetes-free deployment** — Runs standalone on bare-metal or VMs. Local file or etcd as the config backend — no Kubernetes required.
- **Federated cluster design** — A stateless Center connects multiple Controllers across regions, enabling cluster-wide and region-level advanced routing control.

## Plugins

25+ built-in plugins via [EdgionPlugins](docs/en/user-guide/http-route/filters/overview.md) CRD, composable through [Plugin Composition](docs/en/user-guide/http-route/filters/plugin-composition.md):

**Authentication** — [Basic Auth](docs/en/user-guide/http-route/filters/edgion-plugins/basic-auth.md) · [JWT Auth](docs/en/user-guide/http-route/filters/edgion-plugins/jwt-auth.md) · [Key Auth](docs/en/user-guide/http-route/filters/edgion-plugins/key-auth.md) · [HMAC Auth](docs/en/user-guide/http-route/filters/edgion-plugins/hmac-auth.md) · [LDAP Auth](docs/en/user-guide/edgion-plugins/ldap-auth.md) · [Forward Auth](docs/en/user-guide/edgion-plugins/forward-auth.md) · [OpenID Connect](docs/en/user-guide/edgion-plugins/openid-connect.md) · [JWE Decrypt](docs/en/user-guide/edgion-plugins/jwe-decrypt.md) · [Header Cert Auth](docs/en/user-guide/http-route/filters/edgion-plugins/header-cert-auth.md)

**Security** — [CORS](docs/en/user-guide/http-route/filters/edgion-plugins/cors.md) · [CSRF](docs/en/user-guide/http-route/filters/edgion-plugins/csrf.md) · [IP Restriction](docs/en/user-guide/http-route/filters/edgion-plugins/ip-restriction.md) · [Request Restriction](docs/en/user-guide/http-route/filters/edgion-plugins/request-restriction.md)

**Traffic Management** — [Rate Limit](docs/en/user-guide/http-route/filters/edgion-plugins/rate-limit.md) · [Rate Limit (Redis)](docs/en/user-guide/edgion-plugins/rate-limit.md) · [Proxy Rewrite](docs/en/user-guide/http-route/filters/edgion-plugins/proxy-rewrite.md) · [Response Rewrite](docs/en/user-guide/http-route/filters/edgion-plugins/response-rewrite.md) · [Bandwidth Limit](docs/en/user-guide/http-route/filters/edgion-plugins/bandwidth-limit.md) · [Request Mirror](docs/en/user-guide/http-route/filters/edgion-plugins/request-mirror.md) · [Direct Endpoint](docs/en/user-guide/http-route/filters/edgion-plugins/direct-endpoint.md) · [Dynamic Upstream](docs/en/user-guide/http-route/filters/edgion-plugins/dynamic-upstream.md)

**Observability & Utilities** — [Real IP](docs/en/user-guide/edgion-plugins/real-ip.md) · [Ctx Setter](docs/en/user-guide/edgion-plugins/ctx-setter.md) · [Mock](docs/en/user-guide/http-route/filters/edgion-plugins/mock.md) · [DSL](docs/en/user-guide/http-route/filters/edgion-plugins/dsl.md)

**Gateway API Standard Filters** — [Request Header Modifier](docs/en/user-guide/http-route/filters/gateway-api/request-header-modifier.md) · [Response Header Modifier](docs/en/user-guide/http-route/filters/gateway-api/response-header-modifier.md) · [Request Redirect](docs/en/user-guide/http-route/filters/gateway-api/request-redirect.md) · [URL Rewrite](docs/en/user-guide/http-route/filters/gateway-api/url-rewrite.md)

**Stream Plugins (TCP/UDP)** — [IP Restriction](docs/en/user-guide/tcp-route/stream-plugins.md)

## Documentation

- 🇨🇳 [Chinese Documentation](docs/zh-CN/README.md)
- 🇬🇧 [English Documentation](docs/en/README.md)

## Getting Started

### Kubernetes

```bash
# One-line deploy (installs CRDs, controller, gateway, and base config)
deploy/kubernetes/scripts/deploy.sh -y
```

See the [Kubernetes Deployment Guide](deploy/kubernetes/README.md) for configuration options, RBAC setup, and customization.

### Standalone (Bare-Metal / VM)

```bash
# Start controller (file-system config mode) and gateway
deploy/standalone/start.sh
```

See the [Standalone Deployment Guide](deploy/standalone/README.md) for binary installation, TOML configuration, and production tuning.

For usage details, see the [User Guide](docs/en/user-guide/README.md) and the [examples](examples/README.md).

## Testing

```bash
# Run unit tests
cargo test --all --tests

# Run integration tests
./examples/test/scripts/integration/run_integration.sh

# Run a focused integration suite
./examples/test/scripts/integration/run_integration.sh --no-prepare -r HTTPRoute -i Basic
```

## Architecture

Edgion follows a **Controller–Gateway** separation architecture connected via gRPC:

- **Controller** (`edgion-controller`) — Watches configuration sources (Kubernetes CRDs or local YAML), validates and pre-parses resources, then streams them to gateways via gRPC. Handles ACME certificates and status updates.
- **Gateway** (`edgion-gateway`) — Stateless data plane built on Pingora. Receives configuration from the controller, executes routing, plugin chains, load balancing, TLS termination, and access logging.
- **CLI** (`edgion-cli`) — Management tool for inspecting and operating both controller and gateway.

```
                ┌──────────────┐
                │  K8s API /   │
                │  Local YAML  │
                └──────┬───────┘
                       │ watch
                ┌──────▼───────┐
                │  Controller  │ ── Admin API :5800
                └──────┬───────┘
                       │ gRPC :50051
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

## Acknowledgments

- [Pingora](https://github.com/cloudflare/pingora) — High-performance proxy framework by Cloudflare
- [Gateway API](https://gateway-api.sigs.k8s.io/) — Kubernetes SIG Network
- [kube-rs](https://kube.rs/) — Kubernetes client library for Rust
- [nom](https://github.com/rust-bakery/nom) — Parser combinator framework for Rust

## License

Edgion is licensed under the [Apache License, Version 2.0](LICENSE).
