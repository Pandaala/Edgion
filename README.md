# Edgion

A high-performance Kubernetes Gateway built on [Pingora](https://github.com/cloudflare/pingora) and [Gateway API](https://gateway-api.sigs.k8s.io/).

[![Rust](https://img.shields.io/badge/rust-1.75%2B-blue.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

## Features

- **High Performance** - Built on Cloudflare's Pingora framework
- **Gateway API v1** - Full support for the Kubernetes Gateway API standard
- **Extensible Plugin System** - TCP/UDP stream plugins and HTTP filters
- **Advanced TLS** - mTLS and dynamic certificate loading
- **Security** - IP restriction, Basic Auth, CORS, and CSRF protection
- **Observability** - Access logs, metrics, and distributed tracing readiness
- **Protocol Support** - HTTP/1.1, HTTP/2, HTTP/3, gRPC, TCP, UDP, WebSocket, and SNI proxy

## Why Edgion?

| Advantage | Description |
|-----------|-------------|
| **Hot Reload** | All configurations (Routes, Backends, Plugins, TLS Certs) take effect immediately without restart |
| **Dual Deployment** | Works seamlessly in both Kubernetes and bare-metal environments |
| **Gateway API Native** | Standard Kubernetes Gateway API with broad compatibility |
| **Rust-Powered** | Better debugging with clear stack traces compared to Lua-based proxies |
| **Unified Logging** | Single access log captures all request details, errors, and plugin execution info |

## Documentation

- 🇨🇳 [Chinese Documentation](docs/zh-CN/README.md)
- 🇬🇧 [English Documentation](docs/en/README.md) *(Coming soon)*

## Getting Started

For setup and usage details, see the [User Guide](docs/zh-CN/user-guide/README.md) and the [examples](examples/README.md).

## Testing

```bash
# Run unit tests
cargo test --all --tests

# Run integration tests
cd examples/testing
./run_integration_test.sh
```

## Architecture

Edgion consists of two main components:

- **Controller** (`edgion_controller`) - Watches Kubernetes resources and manages configurations
- **Gateway** (`edgion_gateway`) - High-performance data plane that processes traffic

For architecture details, see [Architecture Overview](docs/zh-CN/dev-guide/architecture-overview.md).

## Roadmap

- [ ] **Gateway API Conformance Testing** - Broader validation against Gateway API conformance suites
- [ ] **Cache Plugin** - Response caching and cache-anything support
- [ ] **Async MQ / Log Gateway** - Asynchronous message queue integration
- [ ] **Full-Chain Reconcile** - End-to-end configuration reconciliation
- [ ] **HTTP/3** - Native HTTP/3 support across the gateway stack
- [ ] **AI Gateway** - AI-specific plugins and policies
- [ ] **Sandbox Gateway** - Isolated gateway environments for controlled execution
- [ ] **AI Mesh** - AI service mesh capabilities
- [ ] **MCP Server** - Model Context Protocol server
- [ ] **AI Workflow Engine** - AI-powered workflow orchestration
- [ ] **WASM Plugins** - WebAssembly plugin support

## A Note

Please excuse the many commits with minimal detail. AI-assisted development is moving faster than I can document every change thoroughly right now.

## Contributing

Contributions are welcome! Please check out:

- [Developer Documentation](docs/zh-CN/dev-guide/README.md)
- [Adding New Resources Guide](docs/zh-CN/dev-guide/add-new-resource-guide.md)

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## Acknowledgments

- [Pingora](https://github.com/cloudflare/pingora) - High-performance proxy framework by Cloudflare
- [Gateway API](https://gateway-api.sigs.k8s.io/) - Kubernetes SIG Network
- [kube-rs](https://kube.rs/) - Kubernetes client library for Rust

---

**Version**: v0.1.0  
**Last Updated**: 2026-01-05
