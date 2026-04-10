# HTTPRoute Overview

HTTPRoute is the core resource in Gateway API for defining HTTP routing rules.

## Basic Structure

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: default
spec:
  parentRefs:           # Which Gateway to bind to
    - name: my-gateway
      sectionName: http
  hostnames:            # Hostnames to match
    - "example.com"
  rules:                # List of routing rules
    - matches:          # Match conditions
        - path:
            type: PathPrefix
            value: /api
      filters:          # Filters (optional)
        - type: RequestHeaderModifier
          requestHeaderModifier:
            add:
              - name: X-Custom-Header
                value: "value"
      backendRefs:      # Backend services
        - name: api-service
          port: 8080
          weight: 100
```

Key fields:

| Field | Description |
|-------|-------------|
| `parentRefs` | Binds the route to one or more Gateway listeners |
| `hostnames` | Matches the Host header of requests (exact or wildcard) |
| `rules` | List of routing rules, each containing matches, filters, and backendRefs |

## Core Concepts

### ParentRefs

Specifies which listener of which Gateway this route binds to:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Gateway name |
| `namespace` | String | No | Gateway namespace (defaults to the route's namespace) |
| `sectionName` | String | No | Listener name |

A single HTTPRoute can reference multiple parentRefs to share routing rules across Gateways.

### Rules

Each Rule is an independent routing decision unit containing match conditions, filters, and backend references. Multiple Rules are evaluated in declaration order; the first matching Rule takes effect.

### Matches

Defines request match conditions across multiple dimensions:

- **Path matching**: `PathPrefix`, `Exact`, `RegularExpression`
- **Header matching**: Exact or regex matching on HTTP headers
- **Query parameter matching**: Matching on URL query parameters
- **Method matching**: Matching on HTTP methods (GET, POST, etc.)

Conditions within the same Match are ANDed; multiple Matches are ORed. See [Match Rules](./matches/README.md) for details.

### Filters / Plugins

Executes processing logic before requests reach the backend or before responses are returned to the client:

- **Gateway API standard Filters**: `RequestHeaderModifier`, `ResponseHeaderModifier`, `RequestRedirect`, `URLRewrite`, `RequestMirror`, etc.
- **Edgion extension Plugins**: Reference `EdgionPlugins` CRDs via `ExtensionRef` for advanced features such as rate limiting, authentication, and CORS.

See [Filters](./filters/README.md) for details.

### BackendRefs

Defines the list of backend services for traffic forwarding:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Service name |
| `port` | Integer | Yes | Port number |
| `weight` | Integer | No | Weight (default `1`), used for weighted round-robin |

Supports multi-backend weighted traffic splitting for canary releases and blue-green deployments. See [Backend Configuration](./backends/README.md) for details.

## Full Example

The following example demonstrates an HTTPRoute with multiple rules, covering path matching, header modification, traffic splitting, and more:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: app-route
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
      sectionName: http
  hostnames:
    - "app.example.com"
  rules:
    # Rule 1: API routing with custom header
    - matches:
        - path:
            type: PathPrefix
            value: /api/v2
      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            add:
              - name: X-API-Version
                value: "v2"
      backendRefs:
        - name: api-v2
          port: 8080

    # Rule 2: Canary release, 90% to stable, 10% to canary
    - matches:
        - path:
            type: PathPrefix
            value: /api/v1
      backendRefs:
        - name: api-v1-stable
          port: 8080
          weight: 90
        - name: api-v1-canary
          port: 8080
          weight: 10

    # Rule 3: Redirect old path
    - matches:
        - path:
            type: Exact
            value: /old-page
      filters:
        - type: RequestRedirect
          requestRedirect:
            path:
              type: ReplaceFullPath
              replaceFullPath: /new-page
            statusCode: 301

    # Rule 4: Default backend
    - backendRefs:
        - name: frontend
          port: 3000
```

## Sub-document Navigation

| Topic | Document | Description |
|-------|----------|-------------|
| Match Rules | [matches/](./matches/README.md) | Path, header, query parameter, and method matching |
| Filters | [filters/](./filters/README.md) | Gateway API standard Filters and Edgion extension Plugins |
| Backend Configuration | [backends/](./backends/README.md) | Multi-backend traffic splitting, weights, cross-namespace references |
| Resilience | [resilience/](./resilience/) | Timeout, retry, circuit breaker, and other resilience policies |
