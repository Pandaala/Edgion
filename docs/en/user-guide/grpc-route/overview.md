# GRPCRoute Overview

GRPCRoute is used for routing based on gRPC service name, method name, hostname, and other conditions. It is suitable for managing gRPC traffic between microservices.

## Basic Structure

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: grpc-route
  namespace: production
spec:
  parentRefs:             # Which Gateway to bind to
    - name: main-gateway
      sectionName: grpc
  hostnames:              # Hostnames to match
    - "grpc.example.com"
  rules:                  # List of routing rules
    - matches:            # Match conditions (optional)
        - method:
            service: myapp.UserService
            method: GetUser
      backendRefs:        # Backend services
        - name: user-service
          port: 50051
```

Key fields:

| Field | Description |
|-------|-------------|
| `parentRefs` | Binds the route to one or more Gateway listeners |
| `hostnames` | Matches the `:authority` header of requests (exact or wildcard) |
| `rules` | List of routing rules, each containing matches, filters, and backendRefs |

## Core Concepts

### ParentRefs

Specifies which listener of which Gateway this route binds to. GRPCRoute typically binds to HTTPS or gRPC protocol listeners:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Gateway name |
| `namespace` | String | No | Gateway namespace (defaults to the route's namespace) |
| `sectionName` | String | No | Listener name |

### Rules

Each Rule is an independent routing decision unit containing match conditions, filters, and backend references. Matching rules follow the [precedence rules](matches/overview.md#precedence); declaration order is a final tie-break within one route.

### Matches

GRPCRoute supports matching on gRPC methods:

- **method.service**: Matches gRPC service name (e.g., `myapp.UserService`)
- **method.method**: Matches gRPC method name (e.g., `GetUser`)
- **method.type**: Match type, `Exact` (default) or `RegularExpression`
- **headers**: Matches gRPC metadata headers

Conditions within the same Match are ANDed; multiple Matches are ORed. See [Match Rules](./matches/overview.md) for details.

### Filters / Plugins

Executes processing logic before requests reach the backend:

- **Gateway API standard Filters**: `RequestHeaderModifier`, `ResponseHeaderModifier`, `RequestMirror`, etc.
- **Edgion extension Plugins**: Reference `EdgionPlugins` CRDs via `ExtensionRef`

See [Filters](./filters/overview.md) for details.

### BackendRefs

Defines the list of backend services for gRPC traffic forwarding:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Service name |
| `port` | Integer | Yes | gRPC port number |
| `weight` | Integer | No | Weight (default `1`), used for weighted traffic splitting |

See [Backend Configuration](./backends/overview.md) for details.

## Full Example

The following example demonstrates a complete Gateway + GRPCRoute configuration, covering TLS termination, method matching, and multi-backend traffic splitting:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge-gw
  namespace: gateway-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: grpc-https
      protocol: HTTPS
      port: 443
      hostname: grpc.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - group: ""
            kind: Secret
            name: grpc-cert
---
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: app-grpc-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: grpc-https
  hostnames:
    - grpc.example.com
  rules:
    # Rule 1: Exact match on UserService.GetUser
    - matches:
        - method:
            service: myapp.UserService
            method: GetUser
      backendRefs:
        - name: user-service
          port: 50051

    # Rule 2: Match all OrderService methods, canary splitting
    - matches:
        - method:
            service: myapp.OrderService
      backendRefs:
        - name: order-service-stable
          port: 50051
          weight: 90
        - name: order-service-canary
          port: 50051
          weight: 10

    # Rule 3: Default backend
    - backendRefs:
        - name: default-grpc-service
          port: 50051
```

## Sub-document Navigation

| Topic | Document | Description |
|-------|----------|-------------|
| Match Rules | [matches/](./matches/overview.md) | gRPC service name, method name, and metadata header matching |
| Filters | [filters/](./filters/overview.md) | Gateway API standard Filters and Edgion extension Plugins |
| Backend Configuration | [backends/](./backends/overview.md) | Multi-backend traffic splitting and weight configuration |
