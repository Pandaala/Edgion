# Edgion Documentation

Edgion is a high-performance API Gateway built on Pingora and the Kubernetes Gateway API standard. It delivers NGINX/Envoy-level raw performance and OpenResty-like high programmability, all while enjoying the compile-time memory safety guaranteed by Rust. For the full project overview, see the [GitHub README](https://github.com/Pandaala/Edgion/blob/main/README.md).

## Guides

| Guide | Description |
|-------|-------------|
| [Getting Started](./getting-started/README.md) | Installation, first gateway, core concepts |
| [AI Gateway](./ai-gateway/README.md) | OpenAI-compatible LLM routing, credentials, selection, resilience, usage, and cost |
| [User Guide](./user-guide/README.md) | HTTPRoute, GRPCRoute, plugins, load balancing, resilience |
| [Ops Guide](./ops-guide/README.md) | Gateway configuration, TLS, observability, ACME, production operations |
| [Dev Guide](./dev-guide/README.md) | Architecture internals, adding resources/plugins, coding conventions |
