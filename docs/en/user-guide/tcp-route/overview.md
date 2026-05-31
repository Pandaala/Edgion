# TCPRoute Overview

TCPRoute is used for Layer 4 TCP traffic forwarding without parsing HTTP protocol semantics.

## Basic Structure

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: tcp-route
  namespace: production
spec:
  parentRefs:             # Which Gateway to bind to
    - name: main-gateway
      sectionName: tcp
  rules:                  # List of routing rules
    - backendRefs:        # Backend services
        - name: my-tcp-service
          port: 6379
```

Key fields:

| Field | Description |
|-------|-------------|
| `parentRefs` | Binds the route to one or more Gateway TCP listeners |
| `rules` | List of routing rules, each containing backendRefs |

> TCPRoute operates at Layer 4 and does not support Matches or Filters. Routing decisions rely solely on listener bindings.

## Core Concepts

### ParentRefs

Specifies which TCP listener of which Gateway this route binds to:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Gateway name |
| `namespace` | String | No | Gateway namespace (defaults to the route's namespace) |
| `sectionName` | String | No | Listener name |

Each TCP listener typically binds to only one TCPRoute, since there are no Layer 7 match conditions to differentiate traffic.

### Rules

TCPRoute Rules contain only backendRefs, with no matches or filters fields. Each Rule forwards traffic directly to the specified backends.

### BackendRefs

Defines the list of backend services for TCP traffic forwarding:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Service name |
| `port` | Integer | Yes | Port number |
| `weight` | Integer | No | Weight (default `1`), used for weighted traffic splitting |

## Full Example

The following example demonstrates a complete Gateway + TCPRoute configuration, forwarding Redis and MySQL traffic through the gateway:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge-gw
  namespace: gateway-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: tcp-redis
      protocol: TCP
      port: 6379
    - name: tcp-mysql
      protocol: TCP
      port: 3306
---
# Redis route
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: redis-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
          weight: 100
---
# MySQL route
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: mysql-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: tcp-mysql
  rules:
    - backendRefs:
        - name: mysql-primary
          port: 3306
```

## Sub-document Navigation

| Topic | Document | Description |
|-------|----------|-------------|
| Backend Configuration | [backends/](./backends/overview.md) | Backend service references and weight configuration |
| Stream Plugins | [stream-plugins.md](./stream-plugins.md) | Edgion extension plugins for TCP traffic |
