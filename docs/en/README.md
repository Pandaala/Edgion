# Edgion Documentation

Welcome to Edgion - a high-performance gateway built on Pingora and Kubernetes Gateway API.

## Documentation Navigation

### [Getting Started](./getting-started/README.md)

Installation, deployment, and quick start guide.

### [User Guide](./user-guide/README.md)

Route and plugin configuration for application developers:
- HTTPRoute / GRPCRoute / TCPRoute / UDPRoute
- Edgion extension plugins (BasicAuth, CORS, CSRF, IP Restriction, etc.)
- Load balancing algorithms

### [Operations Guide](./ops-guide/README.md)

Gateway configuration for operators:
- Gateway / GatewayClass configuration
- TLS / mTLS configuration
- Access logging and monitoring

### [Developer Guide](./dev-guide/README.md)

Architecture and extension development for developers:
- [Architecture Overview](./dev-guide/architecture-overview.md)
- [Adding New Resource Types](./dev-guide/add-new-resource-guide.md)
- [Annotations Reference](./dev-guide/annotations-guide.md)

---

## Quick Links

| Category | Link | Description |
|----------|------|-------------|
| User Guide | [user-guide/](./user-guide/) | Route and plugin configuration |
| Operations Guide | [ops-guide/](./ops-guide/) | Gateway/TLS configuration |
| Developer Guide | [dev-guide/](./dev-guide/) | Architecture and development |
| Example Configs | [examples/k8stest/](../../examples/k8stest/) | Integration tests and example configurations |

---

## Version

**Current Version**: v0.1.0  
**Last Updated**: 2026-01-09
