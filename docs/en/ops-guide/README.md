# Ops Guide

This section is intended for operators, covering Gateway deployment, configuration, and management.

## Table of Contents

### Gateway Configuration
- [Gateway Resource Overview](./gateway/overview.md): Gateway and listener configuration entry point.
- [GatewayClass Configuration](./gateway/gateway-class.md): Controller binding and parameter management.
- [HTTP to HTTPS Redirect 🔌](./gateway/http-to-https-redirect.md): Gateway-level redirect policy.
- [Preflight Policy 🔌](./gateway/preflight-policy.md): Gateway-level OPTIONS preflight handling.

### Listeners

- [HTTP Listener](./gateway/listeners/http.md): Plain HTTP access.
- [HTTPS Listener](./gateway/listeners/https.md): TLS termination and certificate configuration.
- [TCP Listener](./gateway/listeners/tcp.md): Layer 4 TCP access.
- [gRPC Listener](./gateway/listeners/grpc.md): gRPC over HTTP/HTTPS listeners.

### TLS

- [TLS Termination](./gateway/tls/tls-termination.md): Standard TLS termination configuration.
- [EdgionTls Extension 🔌](./gateway/tls/edgion-tls.md): Extended certificate and TLS capabilities.
- [ACME Auto-Certificate 🔌](./gateway/tls/acme.md): Automatic issuance and renewal.

### Infrastructure

- [Secret Management](./infrastructure/secret-management.md): Certificate and credential management.
- [Cross-Namespace Reference (ReferenceGrant)](./infrastructure/reference-grant.md): Cross-namespace resource authorization.
- [mTLS Configuration](./infrastructure/mtls.md): Mutual TLS between gateway and backends.

### Observability

- [Access Log](./observability/access-log.md): Access log fields and output.
- [Metrics](./observability/metrics.md): Prometheus metrics collection and alerting recommendations.

### Operations Tools

- [edgion-ctl](./edgion-ctl.md): Resource management and debugging CLI.
