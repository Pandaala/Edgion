# Edgion

A high-performance Kubernetes Gateway built on [Pingora](https://github.com/cloudflare/pingora) and [Gateway API](https://gateway-api.sigs.k8s.io/).

[![Rust](https://img.shields.io/badge/rust-1.75%2B-blue.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

## ✨ Features

- 🚀 **High Performance** - Built on Cloudflare's Pingora framework
- 🎯 **Gateway API v1** - Full support for Kubernetes Gateway API standard
- 🔌 **Extensible Plugin System** - TCP/UDP stream plugins and HTTP filters
- 🔒 **Advanced TLS** - mTLS, dynamic certificate loading (SNI-based)
- 🛡️ **Security** - IP restriction, Basic Auth, CORS, CSRF protection
- 📊 **Observability** - Access logs, metrics, distributed tracing ready
- 🌊 **Protocol Support** - HTTP/1.1, HTTP/2, gRPC, TCP, UDP, WebSocket

## 💡 Why Edgion?

| Advantage | Description |
|-----------|-------------|
| **Hot Reload** | All configurations (Routes, Backends, Plugins, TLS Certs) take effect immediately without restart |
| **Dual Deployment** | Works seamlessly in both Kubernetes and bare-metal environments |
| **Gateway API Native** | Standard Kubernetes Gateway API - widely compatible, AI-friendly configuration |
| **Rust-Powered** | Better debugging with clear stack traces compared to Lua-based proxies |
| **Unified Logging** | Single access log captures all request details, errors, and plugin execution info |

## 📚 Documentation

- 🇨🇳 [Chinese Documentation](docs/zh-CN/README.md)
- 🇬🇧 [English Documentation](docs/en/README.md) *(Coming soon)*

## 🚀 Quick Start

### Prerequisites

- Rust 1.75 or higher
- Kubernetes cluster (optional, for integration testing)

### Build

```bash
# Build all components
cargo build --release

# Build specific binary
cargo build --release --bin edgion_controller
cargo build --release --bin edgion_gateway
```

### Run Controller

```bash
# Start the controller
./target/release/edgion_controller --config config/edgion-controller.toml

# Or with custom configuration
./target/release/edgion_controller \
  --log-level info \
  --grpc-listen 127.0.0.1:50061
```

### Run Gateway

```bash
# Start the gateway
./target/release/edgion_gateway --config config/edgion-gateway.toml

# Or connect to controller
./target/release/edgion_gateway \
  --server-addr http://127.0.0.1:50061 \
  --log-level info
```

For detailed usage, see the [User Guide](docs/zh-CN/user-guide/README.md).

## 🧪 Testing

```bash
# Run unit tests
cargo test --all --tests

# Run integration tests
cd examples/testing
./run_integration_test.sh
```

## 🏗️ Architecture

Edgion consists of two main components:

- **Controller** (`edgion_controller`) - Watches Kubernetes resources and manages configurations
- **Gateway** (`edgion_gateway`) - High-performance data plane that processes traffic

For architecture details, see [Architecture Overview](docs/zh-CN/dev-guide/architecture-overview.md).

## 🗺️ Roadmap

- [ ] **Cache Plugin** - Response caching and cache-anything support
- [ ] **Async MQ / Log Gateway** - Asynchronous message queue integration
- [ ] **Full-Chain Reconcile** - End-to-end configuration reconciliation
- [ ] **AI Gateway** - AI-specific plugins and policies
- [ ] **AI Mesh** - AI service mesh capabilities
- [ ] **MCP Server** - Model Context Protocol server
- [ ] **AI Workflow Engine** - AI-powered workflow orchestration
- [ ] **WASM Plugins** - WebAssembly plugin support

## 🤝 Contributing

Contributions are welcome! Please check out:

- [Developer Documentation](docs/zh-CN/dev-guide/README.md)
- [Adding New Resources Guide](docs/zh-CN/dev-guide/add-new-resource-guide.md)

## 📄 License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## 🙏 Acknowledgments

- [Pingora](https://github.com/cloudflare/pingora) - High-performance proxy framework by Cloudflare
- [Gateway API](https://gateway-api.sigs.k8s.io/) - Kubernetes SIG Network
- [kube-rs](https://kube.rs/) - Kubernetes client library for Rust

---

**Version**: v0.1.0  
**Last Updated**: 2026-01-05
