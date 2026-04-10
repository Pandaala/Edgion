# UDPRoute Overview

UDPRoute is used for UDP traffic forwarding, suitable for scenarios such as DNS, log collection, and game communication.

## Basic Structure

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: udp-route
  namespace: default
spec:
  parentRefs:             # Which Gateway to bind to
    - name: my-gateway
      sectionName: udp
  rules:                  # List of routing rules
    - backendRefs:        # Backend services
        - name: my-udp-service
          port: 53
```

Key fields:

| Field | Description |
|-------|-------------|
| `parentRefs` | Binds the route to one or more Gateway UDP listeners |
| `rules` | List of routing rules, each containing backendRefs |

> UDPRoute operates at Layer 4 and does not support Matches or Filters. Routing decisions rely solely on listener bindings.

## Core Concepts

### ParentRefs

Specifies which UDP listener of which Gateway this route binds to:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Gateway name |
| `namespace` | String | No | Gateway namespace (defaults to the route's namespace) |
| `sectionName` | String | No | Listener name |

Each UDP listener typically binds to only one UDPRoute, since there are no Layer 7 match conditions to differentiate traffic.

### Rules

UDPRoute Rules contain only backendRefs, with no matches or filters fields. Each Rule forwards traffic directly to the specified backends.

### BackendRefs

Defines the list of backend services for UDP traffic forwarding:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Service name |
| `port` | Integer | Yes | Port number |
| `weight` | Integer | No | Weight (default `1`), used for weighted traffic splitting |

## Full Example

The following example demonstrates a complete Gateway + UDPRoute configuration, forwarding DNS and syslog traffic through the gateway:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge-gw
  namespace: gateway-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: udp-dns
      protocol: UDP
      port: 53
    - name: udp-syslog
      protocol: UDP
      port: 514
---
# DNS route
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: dns-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: udp-dns
  rules:
    - backendRefs:
        - name: dns-service
          port: 53
---
# Syslog route
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: syslog-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: udp-syslog
  rules:
    - backendRefs:
        - name: syslog-service
          port: 514
```

## Notes

1. UDP is connectionless; retry and session semantics differ from TCP/HTTP.
2. Reliability compensation should be handled at the backend application layer.

## Sub-document Navigation

| Topic | Document | Description |
|-------|----------|-------------|
| Backend Configuration | [backends/](./backends/overview.md) | Backend service references and weight configuration |
| Stream Plugins | [stream-plugins.md](../tcp-route/stream-plugins.md) | Edgion extension plugins for TCP/UDP traffic |
