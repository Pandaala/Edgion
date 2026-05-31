# Gateway

How Edgion implements the Kubernetes Gateway API and how its pieces fit together.

## Overview

Edgion is a Gateway API implementation. You declare intent with standard Gateway API
resources, and the Edgion controller programs the data plane to match. A typical setup has
three layers:

- A **GatewayClass** that selects the Edgion controller and optionally points at tuning
  parameters.
- One or more **Gateway** resources, each declaring **listeners** (port + protocol + TLS).
- **Routes** (`HTTPRoute`, `GRPCRoute`, `TLSRoute`, `TCPRoute`, `UDPRoute`) that attach to
  listeners and define how traffic is matched and forwarded.

Edgion supports HTTP/1.1, HTTP/2, gRPC, TCP, UDP, TLS, and WebSocket, plus a plugin system,
load balancing, and TLS/mTLS.

## How a request flows

1. A client connects to a listener address (`port` + `protocol`).
2. The listener selects a matching route by `hostname` (SNI / Host header) and
   `allowedRoutes`.
3. The route matches the request (path, headers, method, gRPC service/method, SNI) and
   forwards it to a backend.

## Resources at a glance

| Resource | API group | Purpose |
|----------|-----------|---------|
| `GatewayClass` | `gateway.networking.k8s.io/v1` | Selects the Edgion controller; optional `parametersRef`. |
| `Gateway` | `gateway.networking.k8s.io/v1` | Declares listeners (port, protocol, TLS, allowed routes). |
| `HTTPRoute` | `gateway.networking.k8s.io/v1` | Matches and forwards HTTP traffic. |
| `GRPCRoute` | `gateway.networking.k8s.io/v1` | Matches and forwards gRPC (HTTP/2) traffic. |
| `TLSRoute` | `gateway.networking.k8s.io/v1alpha2` | SNI routing for `Passthrough` TLS. |
| `TCPRoute` | `gateway.networking.k8s.io/v1alpha2` | L4 TCP forwarding. |
| `UDPRoute` | `gateway.networking.k8s.io/v1alpha2` | L4 UDP forwarding. |
| `EdgionGatewayConfig` | `edgion.io/v1alpha1` | Cluster-scoped gateway tuning, referenced via `parametersRef`. |

## Minimal example

A GatewayClass plus a Gateway with a single HTTP listener:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: web-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
```

## Edgion extensions

> **🔌 Edgion extension**
>
> These capabilities are specific to Edgion and not part of standard Gateway API.

Edgion adds behavior on top of the standard spec through `edgion.io/*` annotations and the
`EdgionGatewayConfig` resource:

- `edgion.io/http-to-https-redirect` (Gateway annotation) — redirect HTTP requests to HTTPS.
  See [http-to-https-redirect.md](http-to-https-redirect.md).
- `EdgionGatewayConfig.spec.preflightPolicy` — CORS preflight (`OPTIONS`) handling. See
  [preflight-policy.md](preflight-policy.md).

## Related documents

- [GatewayClass](gateway-class.md)
- [Listeners](listeners/README.md)
- [HTTP to HTTPS redirect](http-to-https-redirect.md)
- [Preflight policy](preflight-policy.md)
- [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)
- [TLS routing (Passthrough)](../../user-guide/tls-route/overview.md)
